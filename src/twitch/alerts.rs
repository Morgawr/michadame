//! Video alerts for Twitch events: followers, subscriptions, bits, and raids.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum AlertKind {
    Follow {
        user: String,
    },
    Subscription {
        user: String,
        tier: String,
        months: u32,
        is_gift: bool,
        gifter: Option<String>,
        message: Option<String>,
    },
    Bits {
        user: String,
        amount: u32,
        message: Option<String>,
    },
    Raid {
        user: String,
        viewers: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlertEvent {
    pub id: String,
    pub kind: AlertKind,
}

#[derive(Clone, Debug)]
pub struct ActiveAlert {
    pub event: AlertEvent,
    pub started: Instant,
    pub duration: Duration,
}

impl ActiveAlert {
    /// Progress from 0.0 (just spawned) to 1.0 (fully expired).
    pub fn progress(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        let total = self.duration.as_secs_f32().max(0.1);
        (elapsed / total).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertTestKind {
    Follow,
    Sub,
    GiftSub,
    Bits,
    Raid,
}

pub struct AlertsState {
    pub queue: VecDeque<AlertEvent>,
    pub active: Option<ActiveAlert>,
    pub just_activated: bool,
    recent_ids: VecDeque<String>,
}

impl Default for AlertsState {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            active: None,
            just_activated: false,
            recent_ids: VecDeque::with_capacity(128),
        }
    }
}

impl AlertsState {
    /// Enqueue an alert event, ignoring duplicates within the recent window.
    pub fn enqueue(&mut self, event: AlertEvent) {
        if !event.id.is_empty() {
            if self.recent_ids.iter().any(|id| id == &event.id) {
                return;
            }
            self.recent_ids.push_back(event.id.clone());
            if self.recent_ids.len() > 128 {
                self.recent_ids.pop_front();
            }
        }
        self.queue.push_back(event);
    }

    /// Advances the active alert lifecycle. Pops the next queued alert when none is active.
    pub fn update(&mut self, now: Instant, default_duration: Duration) -> Option<&ActiveAlert> {
        self.just_activated = false;
        if let Some(active) = &self.active {
            if active.progress(now) >= 1.0 {
                self.active = None;
            }
        }

        if self.active.is_none() {
            if let Some(next_event) = self.queue.pop_front() {
                self.active = Some(ActiveAlert {
                    event: next_event,
                    started: now,
                    duration: default_duration,
                });
                self.just_activated = true;
            }
        }

        self.active.as_ref()
    }

    /// Returns true if an alert was newly popped into active state during the last update step.
    pub fn take_just_activated(&mut self) -> bool {
        let res = self.just_activated;
        self.just_activated = false;
        res
    }

    /// Synthesizes and enqueues a test alert for local preview.
    pub fn trigger_test(&mut self, kind: AlertTestKind) {
        let seq = next_alert_seq();
        let event = match kind {
            AlertTestKind::Follow => AlertEvent {
                id: format!("test-follow-{seq}"),
                kind: AlertKind::Follow {
                    user: "NeonRacer".to_string(),
                },
            },
            AlertTestKind::Sub => AlertEvent {
                id: format!("test-sub-{seq}"),
                kind: AlertKind::Subscription {
                    user: "CyberSamurai".to_string(),
                    tier: "Tier 1".to_string(),
                    months: 3,
                    is_gift: false,
                    gifter: None,
                    message: Some("Loving the retro stream vibes!".to_string()),
                },
            },
            AlertTestKind::GiftSub => AlertEvent {
                id: format!("test-giftsub-{seq}"),
                kind: AlertKind::Subscription {
                    user: "PixelKnight".to_string(),
                    tier: "Tier 1".to_string(),
                    months: 1,
                    is_gift: true,
                    gifter: Some("RetroLord".to_string()),
                    message: None,
                },
            },
            AlertTestKind::Bits => AlertEvent {
                id: format!("test-bits-{seq}"),
                kind: AlertKind::Bits {
                    user: "ArcadeHero".to_string(),
                    amount: 500,
                    message: Some("GG WP on that high score!".to_string()),
                },
            },
            AlertTestKind::Raid => AlertEvent {
                id: format!("test-raid-{seq}"),
                kind: AlertKind::Raid {
                    user: "VaporStreamer".to_string(),
                    viewers: 42,
                },
            },
        };
        self.enqueue(event);
    }
}

pub fn next_alert_seq() -> u64 {
    static NEXT_ALERT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT_ALERT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alert_queue_lifecycle() {
        let mut alerts = AlertsState::default();
        let now = Instant::now();
        let duration = Duration::from_secs(4);

        assert!(alerts.update(now, duration).is_none());

        alerts.trigger_test(AlertTestKind::Follow);
        assert_eq!(alerts.queue.len(), 1);

        assert!(alerts.update(now, duration).is_some());
        assert!(alerts.take_just_activated());
        assert!(!alerts.take_just_activated());
        assert_eq!(alerts.queue.len(), 0);
        let active = alerts.active.as_ref().unwrap();
        assert_eq!(
            active.event.kind,
            AlertKind::Follow {
                user: "NeonRacer".to_string()
            }
        );
        assert_eq!(active.progress(now), 0.0);

        let halfway = now + Duration::from_secs(2);
        assert!((active.progress(halfway) - 0.5).abs() < 0.01);

        let finished = now + Duration::from_secs(5);
        assert!(alerts.update(finished, duration).is_none());
    }

    #[test]
    fn test_deduplicates_by_id() {
        let mut alerts = AlertsState::default();

        let ev1 = AlertEvent {
            id: "msg-123".to_string(),
            kind: AlertKind::Follow {
                user: "User1".to_string(),
            },
        };
        let ev2 = AlertEvent {
            id: "msg-123".to_string(),
            kind: AlertKind::Follow {
                user: "User1".to_string(),
            },
        };

        alerts.enqueue(ev1);
        alerts.enqueue(ev2);
        assert_eq!(alerts.queue.len(), 1);
    }
}
