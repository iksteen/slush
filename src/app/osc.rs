use super::App;
use crate::{
    config::{Chains, Diagnostic},
    json_control::os_string,
};

impl App {
    pub(super) fn handle_pane_osc(&mut self) {
        let events: Vec<_> = self
            .panes
            .iter_mut()
            .flat_map(|(id, pane)| {
                std::mem::take(&mut pane.osc_events)
                    .into_iter()
                    .map(|(verb, payload)| (*id, verb, payload))
            })
            .collect();
        for (id, verb, payload) in events {
            if verb == b"purpose" {
                if let Some(n) = self.layout.nodes.get_mut(&id)
                    && !n.purpose_locked
                {
                    n.purpose = super::commands::purpose(&payload);
                }
                continue;
            }
            let result = self.pane_shader(&verb, &payload);
            let reply = match result {
                Ok(None) => {
                    self.chains.remove(&id);
                    b"ok".to_vec()
                }
                Ok(Some(chains)) => {
                    let message = format!(
                        "ok;{} chrome, {} content",
                        chains.chrome.len(),
                        chains.content.len()
                    );
                    self.chains.insert(id, chains);
                    message.into_bytes()
                }
                Err(message) => [b"error;".as_slice(), message.bytes()].concat(),
            };
            if let Some(p) = self.panes.get_mut(&id) {
                let _ =
                    p.write(&[b"\x1b]5577;1;shader-reply;".as_slice(), &reply, b"\x1b\\"].concat());
            }
        }
    }
    fn pane_shader(&self, verb: &[u8], payload: &[u8]) -> Result<Option<Chains>, Diagnostic> {
        let (where_, text) = payload
            .iter()
            .position(|b| *b == b';')
            .map_or((payload, b"".as_slice()), |at| {
                (&payload[..at], &payload[at + 1..])
            });
        if verb == b"shader" && where_.is_empty() {
            return Ok(None);
        }
        if verb == b"shader" && !matches!(where_, b"content" | b"chrome") {
            return Err(Diagnostic::new("where must be content or chrome"));
        }
        if !self.config.boolean("in_band_shaders") {
            return Err(Diagnostic::new("in_band_shaders is off"));
        }
        let mut parser = self.config.clone();
        parser.messages.clear();
        let chains = if verb == b"shader-load" {
            if payload.is_empty() {
                return Err(Diagnostic::new("no path"));
            }
            parser.parse_chain_file(std::path::Path::new(&*os_string(payload)))?
        } else {
            parser
                .parse_chain(text, where_ == b"chrome")
                .map_err(|e| Diagnostic::new(e.to_string()))?
        };
        if let Some(error) = parser.messages.first() {
            return Err(error.clone());
        }
        if verb == b"shader-load" && chains.chrome.is_empty() && chains.content.is_empty() {
            return Err(Diagnostic::new("nothing in it to run"));
        }
        Ok(Some(chains))
    }
}
