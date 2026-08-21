use iced::Task;
use liana_ui::{component::form, widget::*};
use lianad::{
    bwk_electrum::{client::Client, parse_electrum_url, ElectrumScheme},
    config::ElectrumConfig,
};

use crate::{
    installer::{
        context::Context,
        message::{self, Message},
        view, Error,
    },
    node::electrum::ConfigField,
};

#[derive(Clone)]
pub struct DefineElectrum {
    address: form::Value<String>,
    validate_domain: bool,
}

impl Default for DefineElectrum {
    fn default() -> Self {
        Self {
            address: Default::default(),
            validate_domain: true,
        }
    }
}

impl DefineElectrum {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn can_try_ping(&self) -> bool {
        !self.address.value.is_empty() && self.address.valid
    }

    pub fn update(&mut self, message: message::DefineNode) -> Task<Message> {
        if let message::DefineNode::DefineElectrum(msg) = message {
            match msg {
                message::DefineElectrum::ConfigFieldEdited(field, value) => match field {
                    ConfigField::Address => {
                        self.address.value.clone_from(&value); // save the value including any prefix
                        self.address.valid =
                            crate::node::electrum::is_electrum_address_valid(&value);
                    }
                },
                message::DefineElectrum::ValidDomainChanged(v) => self.validate_domain = v,
            };
        };
        Task::none()
    }

    pub fn apply(&mut self, ctx: &mut Context) -> bool {
        if self.can_try_ping() {
            ctx.bitcoin_backend = Some(lianad::config::BitcoinBackend::Electrum(ElectrumConfig {
                addr: self.address.value.clone(),
                validate_domain: self.validate_domain,
            }));
            return true;
        }
        false
    }

    pub fn view(&self) -> Element<'_, Message> {
        view::define_electrum(&self.address, self.validate_domain)
    }

    pub fn ping(&self) -> Result<(), Error> {
        let (host, port, scheme) =
            parse_electrum_url(&self.address.value).map_err(Error::Electrum)?;
        let host = host.ok_or_else(|| Error::Electrum("Missing host.".to_string()))?;
        let port = port.ok_or_else(|| Error::Electrum("Missing port.".to_string()))?;
        // The client reads the scheme back from the address it is given.
        let addr = match scheme {
            ElectrumScheme::Ssl => format!("ssl://{host}"),
            ElectrumScheme::Tcp => host,
        };
        // Creating the client connects, which is what we are checking here.
        if self.validate_domain {
            Client::new(&addr, port)
        } else {
            Client::new_local(&addr, port)
        }
        .map_err(|e| Error::Electrum(e.to_string()))?;
        Ok(())
    }
}
