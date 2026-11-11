//! The icon for each domain value. The icons live with the views, so the
//! model stays free of GPUI.

use gpui_kit::assets::IconName;

use crate::model::data::{Agent, Assignee, Status, StatusType};
use crate::model::scope::Scope;

pub trait HasIcon {
    fn icon(&self) -> IconName;
}

impl HasIcon for Status {
    fn icon(&self) -> IconName {
        match self.kind {
            StatusType::Backlog => IconName::CircleDashed,
            StatusType::Unstarted => IconName::Circle,
            StatusType::Started => IconName::CircleDot,
            StatusType::Completed => IconName::CircleCheck,
            StatusType::Canceled => IconName::CircleX,
        }
    }
}

impl HasIcon for Agent {
    /// Lucide has no brand marks, so these are stand-ins. Claude gets the
    /// sparkles icon and every other agent gets the chip icon.
    fn icon(&self) -> IconName {
        let harness = self.harness.as_deref().unwrap_or(&self.name);
        if harness.to_lowercase().contains("claude") {
            IconName::Sparkles
        } else {
            IconName::Cpu
        }
    }
}

impl HasIcon for Assignee {
    fn icon(&self) -> IconName {
        match self {
            Self::Person(_) => IconName::User,
            Self::Agent(agent) => agent.icon(),
        }
    }
}

impl HasIcon for Scope {
    fn icon(&self) -> IconName {
        match self {
            Self::All => IconName::Inbox,
            Self::Team(_) => IconName::Users,
            Self::Project(_) => IconName::Folder,
            Self::View(_) => IconName::Funnel,
            Self::Trash => IconName::Trash,
            Self::Document(_) => IconName::FileText,
            Self::Inbox => IconName::MessageCircleQuestionMark,
            Self::Settings => IconName::Settings,
        }
    }
}
