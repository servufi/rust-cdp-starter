use serde::{Deserialize, Serialize};

pub type CdpProtocol = Protocol;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Protocol {
    pub version: Version,
    pub domains: Vec<Domain>,
}

impl Protocol {
    pub fn new() -> Protocol {
        Protocol {
            version: Version::default(),
            domains: vec![],
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub major: String,
    pub minor: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Domain {
    pub domain: String,
    pub description: Option<String>,
    pub deprecated: Option<bool>,
    pub experimental: Option<bool>,
    pub dependencies: Option<Vec<String>>,
    pub types: Option<Vec<Type>>,
    pub commands: Vec<Command>,
    pub events: Option<Vec<Event>>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Type {
    pub id: String,
    pub description: Option<String>,
    pub experimental: Option<bool>,
    #[serde(rename = "type")]
    pub type_name: String,
    pub properties: Option<Vec<TypeProperties>>,
    #[serde(rename = "enum")]
    pub enums: Option<Vec<String>>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeProperties {
    pub name: String,
    pub description: Option<String>,
    pub experimental: Option<bool>,
    pub deprecated: Option<bool>,
    pub optional: Option<bool>,
    #[serde(rename = "type")]
    pub type_name: Option<String>,
    #[serde(rename = "$ref")]
    pub ref_type_name: Option<String>,
    pub items: Option<Items>,
    #[serde(rename = "enum")]
    pub enums: Option<Vec<String>>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Items {
    #[serde(rename = "type")]
    pub type_name: Option<String>,
    #[serde(rename = "$ref")]
    pub ref_type_name: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub name: String,
    pub description: Option<String>,
    pub experimental: Option<bool>,
    pub returns: Option<Vec<CommandReturns>>,
    pub parameters: Option<Vec<Parameters>>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandReturns {
    pub name: String,
    pub description: Option<String>,
    pub deprecated: Option<bool>,
    pub optional: Option<bool>,
    #[serde(rename = "type")]
    pub type_name: Option<String>,
    pub items: Option<Items>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameters {
    pub name: String,
    pub description: Option<String>,
    pub experimental: Option<bool>,
    pub deprecated: Option<bool>,
    pub optional: Option<bool>,
    #[serde(rename = "type")]
    pub type_name: Option<String>,
    #[serde(rename = "$ref")]
    pub ref_type_name: Option<String>,
    #[serde(rename = "enum")]
    pub enums: Option<Vec<String>>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub name: String,
    pub description: Option<String>,
    pub experimental: Option<bool>,
    pub deprecated: Option<bool>,
    pub parameters: Option<Vec<Parameters>>,
}
