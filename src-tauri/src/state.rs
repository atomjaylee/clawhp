use std::sync::Mutex;

pub(crate) static FULL_PATH: Mutex<Option<String>> = Mutex::new(None);
pub(crate) static IN_FLIGHT_AGENT_CREATES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub(crate) fn normalize_agent_id_key(id: &str) -> String {
    id.trim().to_ascii_lowercase()
}

pub(crate) struct AgentCreateGuard {
    id: String,
}

impl AgentCreateGuard {
    pub fn acquire(id: &str) -> Result<Self, String> {
        let key = normalize_agent_id_key(id);
        let mut in_flight = IN_FLIGHT_AGENT_CREATES.lock().unwrap();
        if in_flight.iter().any(|existing| existing == &key) {
            return Err(format!(
                "Agent '{}' 正在创建中，请等待当前创建完成后再刷新列表",
                id
            ));
        }
        in_flight.push(key.clone());
        Ok(Self { id: key })
    }
}

impl Drop for AgentCreateGuard {
    fn drop(&mut self) {
        let mut in_flight = IN_FLIGHT_AGENT_CREATES.lock().unwrap();
        in_flight.retain(|existing| existing != &self.id);
    }
}
