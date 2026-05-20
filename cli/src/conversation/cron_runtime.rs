use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use uuid::Uuid;

use crate::conversation::message_queue::{
    MessageQueue, PromptInputMode, QueuePriority,
};

/// Parsed fields of a 5-field cron expression.
#[derive(Debug, Clone)]
pub struct CronFields {
    minute: CronField,
    hour: CronField,
    day_of_month: CronField,
    month: CronField,
    day_of_week: CronField,
}

#[derive(Debug, Clone)]
enum CronField {
    Any,
    List(Vec<u8>),
}

impl CronField {
    fn parse(s: &str, min: u8, max: u8) -> Result<Self, String> {
        if s == "*" {
            return Ok(CronField::Any);
        }
        let mut values = Vec::new();
        for part in s.split(',') {
            let part = part.split('/').next().unwrap_or(part);
            if let Some((lo, hi)) = part.split_once('-') {
                let lo: u8 = lo.parse().map_err(|_| format!("bad range: {lo}"))?;
                let hi: u8 = hi.parse().map_err(|_| format!("bad range: {hi}"))?;
                for v in lo..=hi {
                    if v < min || v > max {
                        return Err(format!("{v} out of range {min}-{max}"));
                    }
                    values.push(v);
                }
            } else {
                let v: u8 = part.parse().map_err(|_| format!("bad value: {part}"))?;
                if v < min || v > max {
                    return Err(format!("{v} out of range {min}-{max}"));
                }
                values.push(v);
            }
        }
        Ok(CronField::List(values))
    }

    fn matches(&self, value: u8) -> bool {
        match self {
            CronField::Any => true,
            CronField::List(vals) => vals.contains(&value),
        }
    }
}

impl CronFields {
    pub fn parse(expr: &str) -> Result<Self, String> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 5 {
            return Err("cron must have 5 fields".into());
        }
        Ok(CronFields {
            minute: CronField::parse(parts[0], 0, 59)?,
            hour: CronField::parse(parts[1], 0, 23)?,
            day_of_month: CronField::parse(parts[2], 1, 31)?,
            month: CronField::parse(parts[3], 1, 12)?,
            day_of_week: CronField::parse(parts[4], 0, 7)?,
        })
    }

    fn matches_now(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs();
        let days_since_epoch = secs / 86400;
        let time_of_day = secs % 86400;
        let hour = (time_of_day / 3600) as u8;
        let minute = ((time_of_day % 3600) / 60) as u8;

        // Day of week: 1970-01-01 was a Thursday (4). Sunday = 0 in cron.
        let dow = ((days_since_epoch + 4) % 7) as u8;

        let (month, day) = approximate_month_day(days_since_epoch);

        self.minute.matches(minute)
            && self.hour.matches(hour)
            && self.day_of_month.matches(day)
            && self.month.matches(month)
            && self.day_of_week.matches(dow)
    }
}

fn approximate_month_day(days: u64) -> (u8, u8) {
    let month_days: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut remaining = days;
    let years_since_1970 = days / 365;
    let leap_days = (years_since_1970 + 1) / 4;
    remaining = remaining.saturating_sub(leap_days);

    let mut month: u8 = 1;
    for (i, md) in month_days.iter().enumerate() {
        if remaining < *md {
            return (month, remaining as u8 + 1);
        }
        remaining -= *md;
        month = (i + 2) as u8;
        if month > 12 {
            month = 1;
        }
    }
    (12, 31)
}

/// A cron job as stored in the runtime. Uses the CronJob from cron_create for the shared fields.
pub struct CronJob {
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub fields: CronFields,
}

pub struct CronRuntime {
    jobs: Arc<Mutex<HashMap<String, CronJob>>>,
    queue: Arc<MessageQueue>,
    wake_rx: watch::Receiver<bool>,
}

impl CronRuntime {
    pub fn new(
        jobs: Arc<Mutex<HashMap<String, CronJob>>>,
        queue: Arc<MessageQueue>,
        wake_rx: watch::Receiver<bool>,
    ) -> Self {
        CronRuntime {
            jobs,
            queue,
            wake_rx,
        }
    }

    /// Run the cron scheduler loop. Call via tokio::spawn.
    pub async fn run(mut self) {
        let tick_interval = Duration::from_secs(30);
        loop {
            tokio::select! {
                _ = tokio::time::sleep(tick_interval) => {}
                _ = self.wake_rx.changed() => {}
            }

            let mut to_remove: Vec<String> = Vec::new();

            {
                let jobs = self.jobs.lock().unwrap();
                for (id, job) in jobs.iter() {
                    if job.fields.matches_now() {
                        self.queue.enqueue(crate::conversation::message_queue::QueuedCommand {
                            value: job.prompt.clone(),
                            mode: PromptInputMode::Prompt,
                            priority: QueuePriority::Later,
                            agent_id: None,
                            is_meta: true,
                            uuid: Uuid::new_v4(),
                        });

                        if !job.recurring {
                            to_remove.push(id.clone());
                        }
                    }
                }
            }

            if !to_remove.is_empty() {
                let mut jobs = self.jobs.lock().unwrap();
                for id in &to_remove {
                    jobs.remove(id);
                }
            }
        }
    }
}
