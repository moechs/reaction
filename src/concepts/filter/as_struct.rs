use serde::{Deserialize, Serialize};

use crate::concepts::{Match, Patterns};

use super::FilterMethodAs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterMethodStruct {
    pub options: FilterMethodStructInner,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterMethodStructInner {
    lang: Lang,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub enum Lang {
    #[serde(rename = "json", alias = "JSON")]
    JSON,
}

impl FilterMethodAs for FilterMethodStruct {
    fn setup(&mut self, config_patterns: &Patterns) -> Result<super::PatternSet, String> {
        todo!()
    }

    fn get_match(&self, line: &str) -> Option<Match> {
        todo!()
    }

    fn hint_regexes(&self) -> Vec<String> {
        todo!()
    }
}
