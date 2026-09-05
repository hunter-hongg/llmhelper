#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum GroupBy {
    #[default]
    Source,
    Project,
    Model,
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
