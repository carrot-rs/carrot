use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};

#[derive(Clone, Copy, Serialize, Deserialize, Debug, Eq, PartialEq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

impl Role {
    pub fn from_proto(role: i32) -> Role {
        match carrot_proto::LanguageModelRole::from_i32(role) {
            Some(carrot_proto::LanguageModelRole::LanguageModelUser) => Role::User,
            Some(carrot_proto::LanguageModelRole::LanguageModelAssistant) => Role::Assistant,
            Some(carrot_proto::LanguageModelRole::LanguageModelSystem) => Role::System,
            None => Role::User,
        }
    }

    pub fn to_proto(self) -> carrot_proto::LanguageModelRole {
        match self {
            Role::User => carrot_proto::LanguageModelRole::LanguageModelUser,
            Role::Assistant => carrot_proto::LanguageModelRole::LanguageModelAssistant,
            Role::System => carrot_proto::LanguageModelRole::LanguageModelSystem,
        }
    }

    pub fn cycle(self) -> Role {
        match self {
            Role::User => Role::Assistant,
            Role::Assistant => Role::System,
            Role::System => Role::User,
        }
    }
}

impl Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::System => write!(f, "system"),
        }
    }
}
