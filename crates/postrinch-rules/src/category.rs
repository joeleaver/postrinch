//! Gmail's inbox categories, as rules name them.

use postrinch_model::SystemLabel;
use serde::{Deserialize, Serialize};

/// A Gmail inbox category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    Primary,
    Social,
    Promotions,
    Updates,
    Forums,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Primary,
        Category::Social,
        Category::Promotions,
        Category::Updates,
        Category::Forums,
    ];

    /// The system label it is.
    pub fn system_label(self) -> SystemLabel {
        match self {
            Category::Primary => SystemLabel::CategoryPersonal,
            Category::Social => SystemLabel::CategorySocial,
            Category::Promotions => SystemLabel::CategoryPromotions,
            Category::Updates => SystemLabel::CategoryUpdates,
            Category::Forums => SystemLabel::CategoryForums,
        }
    }

    /// Its label id, Gmail's (see [`SystemLabel::provider_id`]).
    pub fn label_id(self) -> &'static str {
        self.system_label().provider_id()
    }

    pub fn name(self) -> &'static str {
        match self {
            Category::Primary => "Primary",
            Category::Social => "Social",
            Category::Promotions => "Promotions",
            Category::Updates => "Updates",
            Category::Forums => "Forums",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_category_is_its_system_label() {
        assert_eq!(Category::Primary.label_id(), "CATEGORY_PERSONAL");
        for category in Category::ALL {
            assert!(category.system_label().is_category());
        }
    }
}
