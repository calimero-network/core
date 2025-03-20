use std::time::Duration;

use eyre::Result as EyreResult;
use serde::{Deserialize, Serialize};
use tokio::time;

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitStep {
    #[serde(rename = "durationMs", with = "serde_duration")]
    pub duration: Duration,
    pub r#for: WaitFor,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitFor {
    #[default]
    Broadcast,
    /// Wait the minimum amount of time for consensus to be reached.
    ///
    /// In the ideal case, this should only take ceil(log2(nodes)).
    ///
    /// For example, with 4 nodes:
    ///
    /// sync 1:
    ///   Node 1 => Node2
    /// sync 2:
    ///   Node 1 => Node3
    ///   Node 2 => Node4
    ///
    /// Or with 8 nodes:
    ///
    /// sync 1:
    ///   Node 1 => Node2
    /// sync 2:
    ///   Node 1 => Node3
    ///   Node 2 => Node4
    /// sync 3:
    ///   Node 1 => Node5
    ///   Node 2 => Node6
    ///   Node 3 => Node7
    ///   Node 4 => Node8
    Consensus,
}

mod serde_duration {
    use core::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_millis() as u64)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        u64::deserialize(deserializer).map(Duration::from_millis)
    }
}

impl Test for WaitStep {
    fn display_name(&self) -> String {
        format!("wait ({:?})", self.r#for)
    }

    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        let mut extra = String::new();

        let factor = match self.r#for {
            WaitFor::Consensus => {
                let nodes = (ctx.invitees.len() + 1) as f64;
                let pairs = nodes.log2().ceil() as u32;
                extra = format!(" (assuming we reach consensus in {} rounds)", pairs);
                pairs
            }
            _ => 1,
        };

        let duration = self.duration * factor;

        ctx.output_writer
            .write_str(&format!("Waiting for {} ms{extra}", duration.as_millis()));

        time::sleep(duration).await;

        Ok(())
    }
}
