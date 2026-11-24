use crate::{
    capture::{self, AudioSource},
    gemini::Gemini,
    protocol::{public_event, Provider, Settings},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, watch, Barrier};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{HeaderName, HeaderValue},
        Message,
    },
    MaybeTlsStream, WebSocketStream,
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub fn emit(app: &AppHandle, id: &str, mut event: Value) {
    event["sessionId"] = json!(id);
    let _ = app.emit("meeting-event", event);
}

async fn send(socket: &mut Socket, event: Value) -> Result<(), String> {
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(Message::Text(event.to_string().into())),
    )
    .await
    .map_err(|_| {
        "Realtime upload stalled. Recording stopped to avoid delayed replies.".to_string()
    })?
    .map_err(|_| "Realtime service disconnected while receiving meeting data.".to_string())
}

pub async fn run(
    app: AppHandle,
    id: String,
    settings: Settings,
    stop: watch::Receiver<bool>,
) -> Result<(), String> {
    emit(&app, &id, json!({"kind":"status","status":"connecting"}));
    let barrier = Arc::new(Barrier::new(2));
    let (to_meeting, meeting_context) = mpsc::channel(32);
    let (to_microphone, microphone_context) = mpsc::channel(32);
    let (ready, mut readiness) = mpsc::channel(2);
    let mut progress_stop = stop.clone();
    let progress = async {
        for _ in 0..2 {
            tokio::select! {
                item = readiness.recv() => {
                    if item.is_none() { return Ok::<_, String>(()); }
                }
                _ = progress_stop.changed() => return Ok(()),
            }
        }
        if !*progress_stop.borrow() {
            emit(&app, &id, json!({"kind":"status","status":"listening"}));
            let _ = progress_stop.changed().await;
        }
        Ok(())
    };
    tokio::try_join!(
        run_source(
            &app,
            &id,
            &settings,
            AudioSource::Meeting,
            stop.clone(),
            barrier.clone(),
            ready.clone(),
            to_microphone,
            meeting_context
        ),
        run_source(
            &app,
            &id,
            &settings,
            AudioSource::Microphone,
            stop,
            barrier,
            ready,
            to_meeting,
            microphone_context
        ),
        progress
    )?;
    Ok(())
}

async fn run_source(
    app: &AppHandle,
    id: &str,
    settings: &Settings,
    source: AudioSource,
    mut stop: watch::Receiver<bool>,
    barrier: Arc<Barrier>,
    ready: mpsc::Sender<()>,
    peer: mpsc::Sender<String>,
    mut context: mpsc::Receiver<String>,
) -> Result<(), String> {
    if *stop.borrow() {
        return Ok(());
    }
    let startup = async {
        let mut url = settings.validate()?;
        if settings.provider == Provider::Gemini {
            // Google requires the key on the handshake URL; never emit or persist this URL.
            url.query_pairs_mut()
                .append_pair("key", settings.token.trim());
        }
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|_| "Invalid WebSocket endpoint.".to_string())?;
        if settings.provider == Provider::Openai {
            let (name, token) = match settings.auth_header.as_str() {
                "bearer" => ("authorization", format!("Bearer {}", settings.token.trim())),
                "api-key" => ("api-key", settings.token.trim().to_string()),
                _ => ("x-api-key", settings.token.trim().to_string()),
            };
            request.headers_mut().insert(
                HeaderName::from_static(name),
                HeaderValue::from_str(&token).map_err(|_| "Invalid proxy token.".to_string())?,
            );
        }
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(20), connect_async(request)).await
            .map_err(|_| "Realtime connection timed out. Check the Realtime URL and Windows network access.".to_string())?
            .map_err(|e| match e {
                tokio_tungstenite::tungstenite::Error::Http(response) => format!("Realtime WebSocket handshake returned HTTP {}. Check authentication and your key in Settings.", response.status()),
                _ => "Could not connect to the Realtime service. Check the endpoint and Windows network access.".into(),
            })?;
        send(&mut socket, settings.session_update(source)).await?;
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match socket.next().await {
                    Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                        let event: Value = serde_json::from_slice(&message.into_data())
                            .map_err(|_| "Realtime service sent invalid JSON.".to_string())?;
                        if event["type"] == "session.updated"
                            || event.get("setupComplete").is_some()
                        {
                            return Ok(());
                        }
                        if event["type"] == "error" || event.get("error").is_some() {
                            return Err(api_error(&event, &settings.token));
                        }
                    }
                    Some(Ok(Message::Ping(bytes))) => {
                        socket
                            .send(Message::Pong(bytes))
                            .await
                            .map_err(|_| "Realtime service disconnected.".to_string())?;
                    }
                    Some(Ok(Message::Close(frame))) => {
                        return Err(close_error(frame, &settings.token))
                    }
                    None => {
                        return Err("Realtime service closed before confirming the session.".into())
                    }
                    Some(Err(_)) => {
                        return Err("Realtime service disconnected during session setup.".into())
                    }
                    _ => {}
                }
            }
        })
        .await
        .map_err(|_| "Realtime service did not confirm session setup.".to_string())??;
        if source == AudioSource::Meeting && !settings.history.is_empty() {
            send(
                &mut socket,
                settings.text_context(&format!(
                    "Recent spoken history from this session, with source labels:\n{}",
                    settings.history
                )),
            )
            .await?;
        }
        Ok::<_, String>(socket)
    };
    let mut socket = tokio::select! {
        result = startup => result?,
        _ = stop.changed() => return Ok(()),
    };
    tokio::select! {
        _ = barrier.wait() => {},
        _ = stop.changed() => return Ok(()),
    }
    if *stop.borrow() {
        return Ok(());
    }
    let device = if source == AudioSource::Meeting {
        &settings.device_id
    } else {
        &settings.microphone_device_id
    };
    let mut capture = capture::start(device.clone(), source);
    let mut announced = false;
    let mut response_turns = VecDeque::new();
    let mut pending_transcripts = HashSet::new();
    let mut pending_replies = HashSet::new();
    let mut speaking = false;
    let mut stopping = false;
    let mut stopped_at = tokio::time::Instant::now();
    let mut last_received = tokio::time::Instant::now();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    let mut drain = tokio::time::interval(Duration::from_millis(100));
    let mut chunks = 0;
    let mut gemini = Gemini::new(source);
    loop {
        tokio::select! {
            audio = capture.audio.recv(), if !stopping => {
                let audio = audio.ok_or("Audio capture ended. Restart recording.")??;
                if !announced { let _ = ready.send(()).await; announced = true; }
                chunks += 1;
                if chunks % 5 == 0 {
                    let peak = audio.chunks_exact(2).map(|b| i16::from_le_bytes([b[0],b[1]]).unsigned_abs() as f64 / 32768.0).fold(0.0_f64, f64::max);
                    emit(app,id,json!({"kind":"level","source":source,"level":peak}));
                }
                send(&mut socket,settings.audio_event(&audio)).await?;
            }
            _ = stop.changed(), if !stopping => {
                capture.stop();
                while let Ok(audio) = capture.audio.try_recv() {
                    send(&mut socket,settings.audio_event(&audio?)).await?;
                }
                // Let VAD commit the final speech without inventing an empty manual turn.
                let silence = vec![0u8; 48 * (settings.silence_ms as usize + 300)];
                send(&mut socket,settings.audio_event(&silence)).await?;
                if settings.provider == Provider::Gemini {
                    send(&mut socket,json!({"realtimeInput":{"audioStreamEnd":true}})).await?;
                }
                stopping = true;
                stopped_at = tokio::time::Instant::now();
            }
            text = context.recv(), if !stopping && source == AudioSource::Meeting => {
                if let Some(text) = text {
                    send(&mut socket,settings.text_context(&format!("{} transcript:\n{}",AudioSource::Microphone.label(),text))).await?;
                }
            }
            message = socket.next() => {
                last_received = tokio::time::Instant::now();
                match message {
                    Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                        let event: Value = serde_json::from_slice(&message.into_data()).map_err(|_| "Realtime service sent invalid JSON.".to_string())?;
                        let events = if settings.provider == Provider::Gemini { gemini.events(event) } else { vec![event] };
                        for event in events {
                            let kind = event["type"].as_str().unwrap_or("");
                            if kind == "error" || kind == "conversation.item.input_audio_transcription.failed" {
                                return Err(api_error(&event,&settings.token));
                            }
                            if kind == "response.done" && event["response"]["status"] == "failed" {
                                return Err(api_error(&json!({"error":event["response"]["status_details"]["error"]}),&settings.token));
                            }
                            if kind == "input_audio_buffer.speech_started" { speaking=true; }
                            if kind == "input_audio_buffer.speech_stopped" { speaking=false; }
                            if kind == "input_audio_buffer.speech_started" || kind == "input_audio_buffer.committed" {
                                if let Some(turn) = event["item_id"].as_str() { pending_transcripts.insert(turn.to_string()); }
                            }
                            if kind == "response.created" {
                                if let Some(reply)=event["response"]["id"].as_str() { pending_replies.insert(reply.to_string()); }
                            }
                            if kind == "response.done" {
                                if let Some(reply)=event["response"]["id"].as_str() { pending_replies.remove(reply); }
                            }
                            if let Some(public)=public_event(&event,source,&mut response_turns) { emit(app,id,public); }
                            if kind == "conversation.item.input_audio_transcription.completed" {
                                if let Some(turn)=event["item_id"].as_str() { pending_transcripts.remove(turn); }
                                if source == AudioSource::Microphone && !stopping && !*stop.borrow() {
                                    if let Some(transcript)=event["transcript"].as_str() {
                                        peer.try_send(transcript.to_string()).map_err(|_| "Transcript context synchronization stopped. Restart recording.".to_string())?;
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(bytes))) => { socket.send(Message::Pong(bytes)).await.map_err(|_| "Realtime service disconnected.".to_string())?; }
                    Some(Ok(Message::Close(frame))) => return Err(close_error(frame, &settings.token)),
                    None => return Err("Realtime session closed. Restart recording to reconnect.".into()),
                    Some(Err(_)) => return Err("Realtime connection interrupted. Restart recording to reconnect.".into()),
                    _ => {}
                }
            }
            _ = drain.tick(), if stopping => {
                if stopped_at.elapsed() >= Duration::from_secs(2) && !speaking && pending_transcripts.is_empty() && pending_replies.is_empty() {
                    let _ = socket.close(None).await;
                    return Ok(());
                }
                if stopped_at.elapsed() > Duration::from_secs(15) {
                    return Err("Recording stopped, but final transcripts or suggestions did not finish. Unfinished turns are marked in exports.".into());
                }
            }
            _ = heartbeat.tick(), if !stopping => {
                if last_received.elapsed() > Duration::from_secs(60) { return Err("Realtime service stopped responding. Restart recording to reconnect.".into()); }
                socket.send(Message::Ping(Vec::new().into())).await.map_err(|_| "Realtime service disconnected.".to_string())?;
            }
        }
    }
}

fn api_error(event: &Value, token: &str) -> String {
    let detail = event["error"]["message"].as_str().unwrap_or(
        "The Realtime service rejected the request. Check model and transcription access.",
    );
    format!(
        "Realtime: {}",
        detail
            .replace(token.trim(), "[redacted]")
            .chars()
            .take(600)
            .collect::<String>()
    )
}

fn close_error(
    frame: Option<tokio_tungstenite::tungstenite::protocol::CloseFrame>,
    token: &str,
) -> String {
    let detail = frame
        .map(|frame| {
            format!(
                "Connection closed ({}): {}. Restart recording to reconnect.",
                frame.code, frame.reason
            )
        })
        .unwrap_or_else(|| "Connection closed. Restart recording to reconnect.".into());
    api_error(&json!({"error":{"message":detail}}), token)
}
