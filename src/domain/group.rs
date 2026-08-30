#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupBy {
    Source,
    Project,
    Model,
}

impl Default for GroupBy {
    fn default() -> Self {
        Self::Source
    }
}

impl GroupBy {
    pub fn next(&self) -> Self {
        match self {
            Self::Source => Self::Project,
            Self::Project => Self::Model,
            Self::Model => Self::Source,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Project => "project",
            Self::Model => "model",
        }
    }
}
