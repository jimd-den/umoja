//! Agent-to-agent messaging.
//!
//! Delivery is decided by what the target is doing, not by what the sender
//! hopes. An idle target receives immediately; a busy one is either steered
//! (interrupted on purpose) or queued behind its current turn. The sender is
//! told which happened, because "delivered" and "queued" lead to different next
//! moves.

use std::sync::Arc;

use umoja_domain::message::MessageLimits;
use umoja_domain::prelude::*;
use umoja_domain::transcript::{TranscriptEvent, TranscriptRecord};

use crate::Env;

#[derive(Debug, Clone)]
pub struct Send {
    pub from_selector: String,
    pub role: ReceiverRole,
    /// Required for child, sibling and peer; ignored for parent and broadcast.
    pub to_name: Option<String>,
    pub body: String,
    pub mode: DeliveryMode,
}

/// Who a session may talk to, and how they are related.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterEntry {
    pub name: String,
    pub session_id: String,
    pub role: ReceiverRole,
    pub status: SessionStatus,
}

pub struct MessagingService {
    env: Env,
    sessions: Arc<dyn SessionStore>,
    messages: Arc<dyn MessageStore>,
    registry: Arc<dyn SubagentRegistry>,
    transcript: Arc<dyn TranscriptLog>,
    limits: MessageLimits,
}

impl std::fmt::Debug for MessagingService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MessagingService")
    }
}

impl MessagingService {
    pub fn new(
        env: Env,
        sessions: Arc<dyn SessionStore>,
        messages: Arc<dyn MessageStore>,
        registry: Arc<dyn SubagentRegistry>,
        transcript: Arc<dyn TranscriptLog>,
        limits: MessageLimits,
    ) -> Self {
        Self {
            env,
            sessions,
            messages,
            registry,
            transcript,
            limits,
        }
    }

    /// Everyone the sender may address: its parent, its children, its siblings.
    ///
    /// A broadcast reaches this roster and no further. Being able to shout at
    /// every session on the machine — including somebody else's unrelated work
    /// — is a footgun, not a feature.
    pub fn roster(&self, from_selector: &str) -> Result<Vec<RosterEntry>> {
        let me = self.sessions.resolve(from_selector)?;
        let mut roster = Vec::new();

        if let Some(parent_id) = &me.parent_id {
            if let Ok(parent) = self.sessions.get(parent_id) {
                roster.push(RosterEntry {
                    name: parent.name.clone(),
                    session_id: parent.id.clone(),
                    role: ReceiverRole::Parent,
                    status: parent.status,
                });

                for sibling in self.sessions.children_of(&parent.id)? {
                    if sibling.id != me.id {
                        roster.push(RosterEntry {
                            name: sibling.name,
                            session_id: sibling.id,
                            role: ReceiverRole::Sibling,
                            status: sibling.status,
                        });
                    }
                }
            }
        }

        for child in self.registry.list(&me.id, false)? {
            if !child.status.is_addressable() {
                continue;
            }
            // The child's own session is the authority on whether it is busy.
            // The registry knows it exists; only the session knows if it is
            // mid-turn, and that is what decides queue-versus-deliver.
            let status = self
                .sessions
                .get(&child.session_id)
                .map(|session| session.status)
                .unwrap_or(SessionStatus::Running);
            roster.push(RosterEntry {
                name: child.name,
                session_id: child.session_id,
                role: ReceiverRole::Child,
                status,
            });
        }

        Ok(roster)
    }

    pub fn send(&self, request: Send) -> Result<Vec<Receipt>> {
        let sender = self.sessions.resolve(&request.from_selector)?;

        if request.role.needs_name() && request.to_name.as_deref().unwrap_or("").trim().is_empty() {
            return Err(DomainError::invalid(format!(
                "a {} message needs a name",
                request.role.label()
            )));
        }

        let roster = self.roster(&sender.id)?;
        let targets: Vec<RosterEntry> = match request.role {
            ReceiverRole::Broadcast => roster,
            ReceiverRole::Parent => roster
                .into_iter()
                .filter(|entry| entry.role == ReceiverRole::Parent)
                .collect(),
            role => {
                let wanted = request.to_name.clone().unwrap_or_default();
                let wanted = Session::normalise_name(&wanted)?;
                roster
                    .into_iter()
                    .filter(|entry| {
                        entry.name == wanted
                            && (role == ReceiverRole::Peer || entry.role == role)
                    })
                    .collect()
            }
        };

        if targets.is_empty() {
            return Err(DomainError::not_found(
                "message target",
                request
                    .to_name
                    .clone()
                    .unwrap_or_else(|| request.role.label().to_string()),
            ));
        }

        let now = self.env.now();
        let mut receipts = Vec::new();

        for target in targets {
            let pending = self.messages.pending_for(&target.session_id)?.len();
            if pending >= self.limits.max_pending_per_target {
                receipts.push(Receipt {
                    message_id: String::new(),
                    receiver_name: target.name.clone(),
                    delivery_status: DeliveryStatus::Failed,
                    note: Some(format!(
                        "{} already has {pending} pending messages",
                        target.name
                    )),
                });
                continue;
            }

            let mut message = AgentMessage::new(
                self.env.id(Ids::MESSAGE),
                &sender.id,
                &sender.name,
                request.role,
                &target.session_id,
                &target.name,
                &request.body,
                request.mode,
                self.limits,
                now,
            )?;

            let status = Self::decide_delivery(request.mode, target.status);
            message.mark(status, now);
            self.messages.enqueue(&message)?;

            self.transcript.append(&TranscriptRecord::new(
                &sender.id,
                now,
                TranscriptEvent::MessageSent {
                    message_id: message.id.clone(),
                    receiver: target.name.clone(),
                    status: status.label().to_string(),
                },
            ))?;

            receipts.push(Receipt {
                message_id: message.id,
                receiver_name: target.name,
                delivery_status: status,
                note: None,
            });
        }

        Ok(receipts)
    }

    fn decide_delivery(mode: DeliveryMode, target: SessionStatus) -> DeliveryStatus {
        match mode {
            // Steering is an explicit request to interrupt: it lands now
            // whatever the target is doing.
            DeliveryMode::Steer => DeliveryStatus::Delivered,
            // A follow-up always waits, even for an idle target, because the
            // sender asked for "after whatever you're doing".
            DeliveryMode::FollowUp => DeliveryStatus::Queued,
            DeliveryMode::Auto => match target {
                SessionStatus::Running => DeliveryStatus::Queued,
                _ => DeliveryStatus::Delivered,
            },
        }
    }

    pub fn inbox(&self, selector: &str) -> Result<Vec<AgentMessage>> {
        let me = self.sessions.resolve(selector)?;
        self.messages.pending_for(&me.id)
    }

    pub fn outbox(&self, selector: &str, limit: Option<usize>) -> Result<Vec<AgentMessage>> {
        let me = self.sessions.resolve(selector)?;
        self.messages.outbox(&me.id, limit)
    }

    /// Reads and retires the inbox. Consuming is destructive on purpose: a
    /// message that stays pending after it has been read would be delivered
    /// again on the next turn.
    pub fn consume(&self, selector: &str) -> Result<Vec<AgentMessage>> {
        let me = self.sessions.resolve(selector)?;
        let now = self.env.now();
        let mut taken = Vec::new();

        for mut message in self.messages.pending_for(&me.id)? {
            message.mark(DeliveryStatus::Consumed, now);
            self.messages.update(&message)?;
            self.transcript.append(&TranscriptRecord::new(
                &me.id,
                now,
                TranscriptEvent::MessageReceived {
                    message_id: message.id.clone(),
                    sender: message.sender_name.clone(),
                },
            ))?;
            taken.push(message);
        }

        Ok(taken)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::doubles::*;
    use crate::sessions::{SessionService, StartSession};
    use crate::subagents::{Spawn, SubagentService};

    struct Fixture {
        messaging: MessagingService,
        subagents: SubagentService,
        sessions_service: SessionService,
        sessions: Arc<MemSessions>,
    }

    fn fixture() -> Fixture {
        let (env, _clock) = env();
        let sessions = Arc::new(MemSessions::default());
        let registry = Arc::new(MemSubagents::default());
        let transcript = Arc::new(MemTranscript::default());
        let messages = Arc::new(MemMessages::default());

        Fixture {
            messaging: MessagingService::new(
                env.clone(),
                sessions.clone(),
                messages,
                registry.clone(),
                transcript.clone(),
                MessageLimits::default(),
            ),
            subagents: SubagentService::new(
                env.clone(),
                sessions.clone(),
                registry,
                Arc::new(MemRunnerRegistry::new(Arc::new(MemRunner::ready()))),
                transcript.clone(),
                DepthPolicy { max_depth: 2 },
            ),
            sessions_service: SessionService::new(
                env,
                sessions.clone(),
                transcript,
                Arc::new(MemSupervisor::default()),
            ),
            sessions,
        }
    }

    fn family(fixture: &Fixture) {
        fixture
            .sessions_service
            .start(StartSession {
                name: Some("root".into()),
                workdir: "/work".into(),
                runner: "claude".into(),
                model: Some("sonnet".into()),
                kind: SessionKind::Root,
                parent: None,
            })
            .unwrap();
        for name in ["api-reviewer", "test-reviewer"] {
            fixture
                .subagents
                .spawn(Spawn {
                    parent_selector: "root".into(),
                    prompt: "review".into(),
                    name: Some(name.into()),
                    model: None,
                    runner: None,
                    system_prompt: None,
                })
                .unwrap();
        }
    }

    fn send(fixture: &Fixture, from: &str, role: ReceiverRole, to: Option<&str>) -> Result<Vec<Receipt>> {
        fixture.messaging.send(Send {
            from_selector: from.into(),
            role,
            to_name: to.map(str::to_string),
            body: "recheck the endpoint".into(),
            mode: DeliveryMode::Auto,
        })
    }

    #[test]
    fn a_parent_can_address_its_child_by_name() {
        let fixture = fixture();
        family(&fixture);
        let receipts = send(&fixture, "root", ReceiverRole::Child, Some("api-reviewer")).unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].receiver_name, "api-reviewer");
        assert_eq!(fixture.messaging.inbox("api-reviewer").unwrap().len(), 1);
    }

    #[test]
    fn a_child_can_reply_to_its_parent_without_naming_it() {
        let fixture = fixture();
        family(&fixture);
        let receipts = send(&fixture, "api-reviewer", ReceiverRole::Parent, None).unwrap();
        assert_eq!(receipts[0].receiver_name, "root");
    }

    #[test]
    fn siblings_can_reach_each_other() {
        let fixture = fixture();
        family(&fixture);
        let receipts = send(
            &fixture,
            "api-reviewer",
            ReceiverRole::Sibling,
            Some("test-reviewer"),
        )
        .unwrap();
        assert_eq!(receipts[0].receiver_name, "test-reviewer");
    }

    #[test]
    fn a_broadcast_stops_at_the_family() {
        let fixture = fixture();
        family(&fixture);
        // An unrelated session that must not receive the broadcast.
        fixture
            .sessions_service
            .start(StartSession {
                name: Some("stranger".into()),
                workdir: "/elsewhere".into(),
                runner: "claude".into(),
                model: None,
                kind: SessionKind::Root,
                parent: None,
            })
            .unwrap();

        let receipts = send(&fixture, "root", ReceiverRole::Broadcast, None).unwrap();
        let names: Vec<&str> = receipts
            .iter()
            .map(|receipt| receipt.receiver_name.as_str())
            .collect();
        assert_eq!(names, vec!["api-reviewer", "test-reviewer"]);
        assert!(fixture.messaging.inbox("stranger").unwrap().is_empty());
    }

    #[test]
    fn a_busy_target_queues_and_an_idle_one_receives() {
        let fixture = fixture();
        family(&fixture);

        let receipts = send(&fixture, "root", ReceiverRole::Child, Some("api-reviewer")).unwrap();
        assert_eq!(receipts[0].delivery_status, DeliveryStatus::Queued);

        let mut child = fixture.sessions.resolve("api-reviewer").unwrap();
        child.status = SessionStatus::Idle;
        fixture.sessions.update(&child).unwrap();

        let receipts = send(&fixture, "root", ReceiverRole::Child, Some("api-reviewer")).unwrap();
        assert_eq!(receipts[0].delivery_status, DeliveryStatus::Delivered);
    }

    #[test]
    fn steering_lands_even_on_busy_work() {
        let fixture = fixture();
        family(&fixture);
        let receipts = fixture
            .messaging
            .send(Send {
                from_selector: "root".into(),
                role: ReceiverRole::Child,
                to_name: Some("api-reviewer".into()),
                body: "stop and read this".into(),
                mode: DeliveryMode::Steer,
            })
            .unwrap();
        assert_eq!(receipts[0].delivery_status, DeliveryStatus::Delivered);
    }

    #[test]
    fn consuming_the_inbox_empties_it() {
        let fixture = fixture();
        family(&fixture);
        send(&fixture, "root", ReceiverRole::Child, Some("api-reviewer")).unwrap();

        let taken = fixture.messaging.consume("api-reviewer").unwrap();
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].status, DeliveryStatus::Consumed);
        assert!(fixture.messaging.inbox("api-reviewer").unwrap().is_empty());
    }

    #[test]
    fn an_unknown_target_is_an_error_not_a_silent_drop() {
        let fixture = fixture();
        family(&fixture);
        assert!(matches!(
            send(&fixture, "root", ReceiverRole::Child, Some("nobody")),
            Err(DomainError::NotFound { .. })
        ));
    }

    #[test]
    fn a_deleted_child_can_no_longer_be_addressed() {
        let fixture = fixture();
        family(&fixture);
        fixture.subagents.delete("root", "api-reviewer").unwrap();
        assert!(send(&fixture, "root", ReceiverRole::Child, Some("api-reviewer")).is_err());
    }
}
