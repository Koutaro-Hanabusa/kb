//! Core library for `kb`: a fast Markdown knowledge base.
//!
//! A knowledge base is a directory of *notebooks*, each of which is a git
//! repository full of Markdown notes. There is no index and no database — every
//! operation walks the tree directly, which stays comfortably fast at the sizes
//! a personal knowledge base reaches.

pub mod bookmark;
pub mod browse;
pub mod create;
pub mod encrypt;
pub mod frontmatter;
pub mod git;
pub mod index;
pub mod items;
pub mod migrate;
pub mod note;
pub mod plugins;
pub mod render;
pub mod search;
pub mod selector;
pub mod settings;
pub mod sync;
pub mod todo;
pub mod workspace;

pub use create::NewNote;
pub use frontmatter::{Document, Frontmatter};
pub use index::Index;
pub use note::Note;
pub use search::{Hit, MatchLine, Query};
pub use selector::{Resolved, Selector, Target};
pub use workspace::{Notebook, Workspace};
