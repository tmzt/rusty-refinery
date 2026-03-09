use redis::AsyncCommands;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EventError {
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
}

pub struct EventStream {
    connection: redis::aio::MultiplexedConnection,
}

#[derive(Debug, Clone)]
pub enum BeadEvent {
    NewBead {
        bead_id: String,
        prd_path: String,
    },
    AgentSpawn {
        bead_id: String,
        pid: u32,
        template: String,
    },
    SigChld {
        bead_id: String,
        pid: u32,
        exit_code: i32,
    },
    Heartbeat {
        bead_id: String,
        status: String,
    },
}

impl BeadEvent {
    fn event_type(&self) -> &'static str {
        match self {
            BeadEvent::NewBead { .. } => "NEW_BEAD",
            BeadEvent::AgentSpawn { .. } => "AGENT_SPAWN",
            BeadEvent::SigChld { .. } => "SIGCHLD",
            BeadEvent::Heartbeat { .. } => "HEARTBEAT",
        }
    }

    fn fields(&self) -> Vec<(&str, String)> {
        match self {
            BeadEvent::NewBead { bead_id, prd_path } => {
                vec![("bead_id", bead_id.clone()), ("prd_path", prd_path.clone())]
            }
            BeadEvent::AgentSpawn {
                bead_id,
                pid,
                template,
            } => vec![
                ("bead_id", bead_id.clone()),
                ("pid", pid.to_string()),
                ("template", template.clone()),
            ],
            BeadEvent::SigChld {
                bead_id,
                pid,
                exit_code,
            } => vec![
                ("bead_id", bead_id.clone()),
                ("pid", pid.to_string()),
                ("exit_code", exit_code.to_string()),
            ],
            BeadEvent::Heartbeat { bead_id, status } => {
                vec![("bead_id", bead_id.clone()), ("status", status.clone())]
            }
        }
    }
}

const STREAM_KEY: &str = "beads:events";

impl EventStream {
    pub async fn connect(redis_url: &str) -> Result<Self, EventError> {
        let client = redis::Client::open(redis_url)?;
        let connection = client.get_multiplexed_async_connection().await?;
        Ok(EventStream { connection })
    }

    pub async fn emit(&mut self, event: BeadEvent) -> Result<(), EventError> {
        let event_type = event.event_type();
        let mut fields = event.fields();
        fields.push(("type", event_type.to_string()));

        let field_pairs: Vec<(&str, String)> = fields;
        redis::cmd("XADD")
            .arg(STREAM_KEY)
            .arg("*")
            .arg(&field_pairs)
            .query_async(&mut self.connection)
            .await
            .map(|_: String| ())?;

        Ok(())
    }

    pub async fn check_bead_status(&mut self, bead_id: &str) -> Result<Option<String>, EventError> {
        let key = format!("bead:status:{bead_id}");
        let result: Option<String> = self.connection.get(&key).await?;
        Ok(result)
    }

    pub async fn set_bead_status(
        &mut self,
        bead_id: &str,
        status: &str,
    ) -> Result<(), EventError> {
        let key = format!("bead:status:{bead_id}");
        self.connection.set::<_, _, ()>(&key, status).await?;
        Ok(())
    }
}
