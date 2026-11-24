use crate::capture::AudioSource;
use serde_json::{json, Value};

#[derive(Default)]
pub struct Gemini {
    sequence: u64,
    input: Option<Input>,
    reply: Option<Reply>,
    speech_ended: bool,
    late_input: bool,
    transcription_only: bool,
}

struct Input {
    id: String,
    text: String,
    done: bool,
}

struct Reply {
    id: String,
    turn_id: String,
    text: String,
}

impl Gemini {
    pub fn new(source: AudioSource) -> Self {
        Self {
            transcription_only: source == AudioSource::Microphone,
            ..Self::default()
        }
    }
    fn start_input(&mut self, events: &mut Vec<Value>) {
        self.sequence += 1;
        let id = format!("gemini-turn-{}", self.sequence);
        events.push(json!({"type":"input_audio_buffer.speech_started","item_id":id}));
        self.input = Some(Input {
            id,
            text: String::new(),
            done: false,
        });
        self.speech_ended = false;
        self.late_input = false;
    }

    fn finish_input(&mut self, events: &mut Vec<Value>) {
        if let Some(input) = &mut self.input {
            events.push(json!({"type":"input_audio_buffer.speech_stopped","item_id":input.id}));
            // Transcription arrives independently; keep empty turns pending for late input.
            if !input.done && !input.text.is_empty() {
                input.done = true;
                events.push(json!({"type":"conversation.item.input_audio_transcription.completed","item_id":input.id,"transcript":input.text}));
            }
        }
    }

    fn finish_reply(&mut self, events: &mut Vec<Value>, status: &str) {
        if let Some(reply) = self.reply.take() {
            if self
                .input
                .as_ref()
                .is_some_and(|input| input.id == reply.turn_id)
            {
                self.finish_input(events);
                self.late_input = self
                    .input
                    .as_ref()
                    .is_some_and(|input| input.text.is_empty());
            }
            if status == "completed" {
                events.push(json!({"type":"response.output_text.done","response_id":reply.id,"text":reply.text}));
            }
            events.push(json!({"type":"response.done","response":{"id":reply.id,"status":status}}));
        }
    }

    // Normalize Gemini into the existing turn protocol, so UI pairing and stop draining are shared.
    pub fn events(&mut self, message: Value) -> Vec<Value> {
        let mut events = Vec::new();
        if message.get("setupComplete").is_some() {
            events.push(json!({"type":"session.updated"}));
        }
        if message.get("error").is_some() {
            events.push(json!({"type":"error","error":message["error"]}));
        }
        if message.get("goAway").is_some() {
            events.push(json!({"type":"error","error":{"message":"Gemini Live session is nearing its connection limit. Restart recording to reconnect."}}));
        }
        let content = &message["serverContent"];
        if self.transcription_only {
            if let Some(text) = content["interimInputTranscription"]["text"]
                .as_str()
                .filter(|text| !text.is_empty())
            {
                if self.input.is_none() || self.input.as_ref().is_some_and(|input| input.done) {
                    self.start_input(&mut events);
                }
                let input = self.input.as_mut().unwrap();
                input.text = text.into();
                events.push(json!({"type":"conversation.item.input_audio_transcription.delta","item_id":input.id,"delta":text,"replace":true}));
            }
            if let Some(text) = content["inputTranscription"]["text"]
                .as_str()
                .filter(|text| !text.is_empty())
            {
                if self.input.is_none() || self.input.as_ref().is_some_and(|input| input.done) {
                    self.start_input(&mut events);
                }
                self.input.as_mut().unwrap().text = text.into();
                self.finish_input(&mut events);
            }
            return events;
        }
        if content["interrupted"] == true {
            self.finish_reply(&mut events, "cancelled");
        }
        match message["voiceActivity"]["type"].as_str() {
            Some("ACTIVITY_START") => {
                if self.input.is_some() && self.speech_ended {
                    self.finish_input(&mut events);
                }
                if self.input.is_none()
                    || self.speech_ended
                    || self.input.as_ref().is_some_and(|input| input.done)
                {
                    self.start_input(&mut events);
                }
            }
            Some("ACTIVITY_END") => {
                self.speech_ended = true;
                if let Some(input) = &self.input {
                    events.push(
                        json!({"type":"input_audio_buffer.speech_stopped","item_id":input.id}),
                    );
                }
            }
            _ => {}
        }
        if let Some(text) = content["inputTranscription"]["text"]
            .as_str()
            .filter(|text| !text.is_empty())
        {
            if self.input.is_none() || self.input.as_ref().is_some_and(|input| input.done) {
                self.start_input(&mut events);
            }
            let input = self.input.as_mut().unwrap();
            input.text.push_str(text);
            events.push(json!({"type":"conversation.item.input_audio_transcription.delta","item_id":input.id,"delta":text}));
            if self.late_input {
                self.finish_input(&mut events);
                self.late_input = false;
            }
        }
        let parts = content["modelTurn"]["parts"].as_array();
        let has_output = content.get("outputTranscription").is_some()
            || parts.is_some_and(|parts| {
                parts.iter().any(|part| {
                    part.get("inlineData").is_some()
                        || (part.get("text").is_some() && part["thought"] != true)
                })
            });
        if has_output {
            if self.reply.is_none() {
                if self.input.is_none()
                    || self.input.as_ref().is_some_and(|input| input.done) && !self.late_input
                {
                    self.start_input(&mut events);
                }
                let turn_id = self.input.as_ref().unwrap().id.clone();
                let id = format!("{turn_id}-reply");
                events.push(json!({"type":"input_audio_buffer.speech_stopped","item_id":turn_id}));
                events.push(json!({"type":"input_audio_buffer.committed","item_id":turn_id}));
                events.push(json!({"type":"response.created","response":{"id":id}}));
                self.reply = Some(Reply {
                    id,
                    turn_id,
                    text: String::new(),
                });
            }
            let reply = self.reply.as_mut().unwrap();
            // Native audio is discarded; only its transcription is shown. Thought text stays private.
            let text = if let Some(text) = content["outputTranscription"]["text"].as_str() {
                text.to_string()
            } else {
                parts
                    .into_iter()
                    .flatten()
                    .filter(|part| part["thought"] != true)
                    .filter_map(|part| part["text"].as_str())
                    .collect::<String>()
            };
            if !text.is_empty() {
                reply.text.push_str(&text);
                events.push(json!({"type":"response.output_text.delta","response_id":reply.id,"delta":text}));
            }
        }
        if content["turnComplete"] == true && content["interactionStatus"] != "IN_PROGRESS" {
            if self.reply.is_some() {
                self.finish_reply(&mut events, "completed");
            } else if content["interrupted"] != true {
                self.finish_input(&mut events);
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{capture::AudioSource, protocol::public_event};
    use std::collections::VecDeque;

    #[test]
    fn pairs_transcripts_and_suggestions_from_combined_and_interleaved_frames() {
        let mut gemini = Gemini::default();
        let mut queue = VecDeque::new();
        let frames = [
            json!({"serverContent":{"inputTranscription":{"text":"Can we "}}}),
            json!({"serverContent":{"modelTurn":{"parts":[{"inlineData":{"data":"discarded-audio"}}]},"outputTranscription":{"text":"I can "},"inputTranscription":{"text":"ship?"}}}),
            json!({"serverContent":{"outputTranscription":{"text":"confirm tomorrow."},"generationComplete":true}}),
            json!({"serverContent":{"turnComplete":true}}),
        ];
        let public: Vec<_> = frames
            .into_iter()
            .flat_map(|frame| gemini.events(frame))
            .filter_map(|event| public_event(&event, AudioSource::Meeting, &mut queue))
            .collect();
        let turn = public
            .iter()
            .find(|event| event["kind"] == "transcript" && event["done"] == true)
            .unwrap();
        assert_eq!(turn["text"], "Can we ship?");
        let reply = public
            .iter()
            .find(|event| event["kind"] == "reply" && event["done"] == true)
            .unwrap();
        assert_eq!(reply["text"], "I can confirm tomorrow.");
        assert_eq!(reply["source"], "meeting");
        assert!(public.iter().any(|event| event["turnId"] == turn["id"]));
        assert!(!serde_json::to_string(&public)
            .unwrap()
            .contains("discarded-audio"));
        assert!(queue.is_empty());
    }

    #[test]
    fn interruptions_end_old_reply_and_new_speech_gets_a_new_turn() {
        let mut gemini = Gemini::default();
        gemini.events(json!({"serverContent":{"inputTranscription":{"text":"First question"},"outputTranscription":{"text":"Try "}}}));
        let interrupted =
            gemini.events(json!({"serverContent":{"interrupted":true,"turnComplete":true}}));
        assert!(interrupted
            .iter()
            .any(|event| event["response"]["status"] == "cancelled"));
        let next = gemini.events(json!({"serverContent":{"inputTranscription":{"text":"Second question"},"outputTranscription":{"text":"Next"},"turnComplete":true}}));
        assert!(next.iter().any(|event| event["type"] == "response.created"
            && event["response"]["id"] == "gemini-turn-2-reply"));
        assert!(next
            .iter()
            .any(|event| event["transcript"] == "Second question"));
    }

    #[test]
    fn late_transcription_keeps_the_original_reply_pairing() {
        let mut gemini = Gemini::default();
        gemini.events(json!({"serverContent":{"outputTranscription":{"text":"Suggestion"},"turnComplete":true}}));
        let late =
            gemini.events(json!({"serverContent":{"inputTranscription":{"text":"Question"}}}));
        assert!(late
            .iter()
            .any(|event| event["transcript"] == "Question" && event["item_id"] == "gemini-turn-1"));
        let next = gemini.events(json!({"serverContent":{"inputTranscription":{"text":"Next question"},"outputTranscription":{"text":"Next suggestion"}}}));
        assert!(next
            .iter()
            .any(|event| event["response"]["id"] == "gemini-turn-2-reply"));
    }

    #[test]
    fn handles_setup_errors_and_background_reasoning_without_leaking_thoughts() {
        let mut gemini = Gemini::default();
        assert_eq!(
            gemini.events(json!({"setupComplete":{}}))[0]["type"],
            "session.updated"
        );
        assert_eq!(
            gemini.events(json!({"error":{"message":"Bad key"}}))[0]["error"]["message"],
            "Bad key"
        );
        assert_eq!(
            gemini.events(json!({"goAway":{"timeLeft":"30s"}}))[0]["type"],
            "error"
        );
        assert!(gemini.events(json!({"serverContent":{"modelTurn":{"parts":[{"thought":true,"text":"private reasoning"}]}}})).is_empty());
        let ongoing = gemini.events(json!({"serverContent":{"inputTranscription":{"text":"Question"},"outputTranscription":{"text":"Suggestion"},"turnComplete":true,"interactionStatus":"IN_PROGRESS"}}));
        assert!(!ongoing.iter().any(|event| event["type"] == "response.done"));
        assert!(gemini
            .events(json!({"serverContent":{"turnComplete":true,"interactionStatus":"IDLE"}}))
            .iter()
            .any(|event| event["type"] == "response.done"));
    }

    #[test]
    fn barge_in_transcripts_do_not_finalize_with_the_previous_reply() {
        let mut gemini = Gemini::default();
        gemini.events(json!({"voiceActivity":{"type":"ACTIVITY_START"}}));
        gemini.events(json!({"serverContent":{"inputTranscription":{"text":"First question"}}}));
        gemini.events(json!({"voiceActivity":{"type":"ACTIVITY_END"}}));
        gemini.events(json!({"serverContent":{"outputTranscription":{"text":"First suggestion"}}}));
        let next = gemini.events(json!({"voiceActivity":{"type":"ACTIVITY_START"},"serverContent":{"inputTranscription":{"text":"Second question"}}}));
        assert!(next
            .iter()
            .any(|event| event["transcript"] == "First question"));
        let interrupted =
            gemini.events(json!({"serverContent":{"interrupted":true,"turnComplete":true}}));
        assert!(!interrupted
            .iter()
            .any(|event| event["transcript"] == "Second question"));
        let reply = gemini.events(json!({"serverContent":{"outputTranscription":{"text":"Second suggestion"},"turnComplete":true}}));
        assert!(reply
            .iter()
            .any(|event| event["response"]["id"] == "gemini-turn-2-reply"));
        assert!(reply
            .iter()
            .any(|event| event["transcript"] == "Second question"));
    }

    #[test]
    fn microphone_transcribes_corrected_hypotheses_without_suggestions() {
        let mut gemini = Gemini::new(AudioSource::Microphone);
        let partial =
            gemini.events(json!({"serverContent":{"interimInputTranscription":{"text":"I can"}}}));
        assert!(partial
            .iter()
            .any(|event| event["delta"] == "I can" && event["replace"] == true));
        let final_events = gemini.events(json!({"serverContent":{"inputTranscription":{"text":"I will check with logistics."},"outputTranscription":{"text":"Unwanted suggestion"}}}));
        assert!(final_events
            .iter()
            .any(|event| event["transcript"] == "I will check with logistics."));
        assert!(!final_events.iter().any(|event| event["type"]
            .as_str()
            .unwrap_or("")
            .starts_with("response.")));
        assert_eq!(gemini.sequence, 1);
        let next = gemini
            .events(json!({"serverContent":{"inputTranscription":{"text":"I have checked."}}}));
        assert!(next
            .iter()
            .any(|event| event["item_id"] == "gemini-turn-2"
                && event["transcript"] == "I have checked."));
    }

    #[test]
    fn observed_live_frames_pair_the_answer_only_with_opposite_speech() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/gemini-live.json")).unwrap();
        for (name, source) in [
            ("meeting", AudioSource::Meeting),
            ("microphone", AudioSource::Microphone),
        ] {
            let mut gemini = Gemini::new(source);
            let mut queue = VecDeque::new();
            let public: Vec<_> = fixture[name]
                .as_array()
                .unwrap()
                .iter()
                .cloned()
                .flat_map(|frame| gemini.events(frame))
                .filter_map(|event| public_event(&event, source, &mut queue))
                .collect();
            let transcript = public
                .iter()
                .find(|event| event["kind"] == "transcript" && event["done"] == true)
                .unwrap();
            assert_eq!(
                transcript["text"],
                "Can you confirm the delivery date for this order?"
            );
            if source == AudioSource::Meeting {
                let answer = public
                    .iter()
                    .find(|event| event["kind"] == "reply" && event["done"] == true)
                    .unwrap();
                assert_eq!(
                    answer["text"],
                    "The confirmed delivery date for the sample order is Friday, December 4, 2026."
                );
                assert!(public
                    .iter()
                    .any(|event| event["kind"] == "reply" && event["turnId"] == transcript["id"]));
            } else {
                assert!(!public
                    .iter()
                    .any(|event| event["kind"] == "reply" || event["kind"] == "reply_end"));
            }
            assert!(queue.is_empty());
            assert!(!serde_json::to_string(&public)
                .unwrap()
                .contains("[discarded]"));
        }
    }
}
