//! Command definitions, mirroring `nb`'s surface.
//!
//! Aliases are the ones `nb` documents, so muscle memory carries over.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "kb",
    version,
    about = "A fast Markdown knowledge base",
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
pub struct Cli {
    /// Knowledge base root (defaults to $KB_ROOT, then ~/.nb)
    #[arg(long, global = true, value_name = "DIR")]
    pub root: Option<PathBuf>,

    /// Item to show. Without one, lists the current notebook.
    #[arg(value_name = "SELECTOR")]
    pub selector: Option<String>,

    /// `kb <id>` is shorthand for `kb show <id>`, so it takes the same options.
    #[command(flatten)]
    pub show: ShowOpts,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create a new note or folder
    #[command(visible_aliases = ["a", "create", "new", "+"])]
    Add(AddArgs),

    /// List items
    #[command(visible_alias = "ls")]
    List(ListArgs),

    /// Search note contents
    #[command(visible_aliases = ["q", "grep"])]
    Search(SearchArgs),

    /// Show an item
    #[command(visible_aliases = ["s", "view"])]
    Show(ShowArgs),

    /// View an item in the terminal
    #[command(visible_aliases = ["p", "preview"])]
    Peek(ShowArgs),

    /// Open an item in the system's preferred application
    #[command(visible_alias = "o")]
    Open(ShowArgs),

    /// Edit an item
    #[command(visible_alias = "e")]
    Edit(EditArgs),

    /// Delete one or more items
    #[command(visible_aliases = ["d", "rm"])]
    Delete(DeleteArgs),

    /// Move or rename an item
    #[command(visible_aliases = ["mv", "rename"])]
    Move(MoveArgs),

    /// Copy or duplicate an item
    #[command(visible_aliases = ["cp", "duplicate"])]
    Copy(CopyArgs),

    /// Print the number of items
    Count(SelectorArgs),

    /// Manage notebooks
    Notebooks(NotebooksArgs),

    /// Switch to a notebook
    #[command(visible_alias = "u")]
    Use(UseArgs),

    /// Print git and remote status for a notebook
    Status(NotebookArgs),

    /// Run a git command in a notebook
    Git(GitArgs),

    /// Show revision history
    History(SelectorArgs),

    /// Add, delete, and list folders
    Folders(FoldersArgs),

    /// Commit changes, then pull and push
    Sync(SyncArgs),

    /// Initialize a knowledge base
    Init(InitArgs),

    /// Rebuild `.index` from what is on disk
    Reconcile(NotebookArgs),

    /// Pick an item with fzf
    Pick(PickArgs),

    /// Show tags and how many notes carry them
    Tags(FilterArgs),

    /// Add missing frontmatter to existing notes
    Migrate(MigrateArgs),

    /// Bookmark a URL, or list bookmarks
    #[command(visible_alias = "bm")]
    Bookmark(BookmarkArgs),

    /// Manage todos
    #[command(visible_aliases = ["todos", "tasks"])]
    Todo(TodoArgs),

    /// Mark a todo done
    Do(SelectorArgs),

    /// Mark a todo not done
    Undo(SelectorArgs),

    /// Pin an item to the top of listings
    Pin(SelectorArgs),

    /// Remove an item's pin
    Unpin(SelectorArgs),

    /// Browse notes in a web browser
    Browse(BrowseArgs),

    /// Archive a notebook
    Archive(NotebookArgs),

    /// Remove a notebook's archived mark
    Unarchive(NotebookArgs),
}

#[derive(Args)]
pub struct BrowseArgs {
    /// Item, folder, or notebook to open
    #[arg(value_name = "SELECTOR")]
    pub selector: Option<String>,

    /// Start the web server and keep it running
    #[arg(short, long, visible_alias = "daemon")]
    pub serve: bool,

    /// Open in the system's web browser
    #[arg(short, long)]
    pub gui: bool,

    /// Print the rendered HTML to standard output
    #[arg(short, long)]
    pub print: bool,

    /// Browse notebooks
    #[arg(short, long)]
    pub notebooks: bool,

    /// Open to the search results for this query
    #[arg(short, long, value_name = "QUERY")]
    pub query: Option<String>,

    /// Search for a tag
    #[arg(short = 't', long, visible_alias = "tags", value_name = "TAG")]
    pub tag: Option<String>,

    /// Port to listen on
    #[arg(long, value_name = "PORT", default_value_t = kb_core::browse::DEFAULT_PORT)]
    pub port: u16,
}

#[derive(Args)]
pub struct TodoArgs {
    #[command(subcommand)]
    pub command: Option<TodoCommand>,

    #[command(flatten)]
    pub filters: FilterArgs,

    /// Show done todos as well
    #[arg(long)]
    pub all: bool,
}

#[derive(Subcommand)]
pub enum TodoCommand {
    /// Add a todo
    Add(TodoAddArgs),
    /// List open todos
    List(TodoListArgs),
    /// Mark a todo done
    Do(SelectorArgs),
    /// Mark a todo not done
    Undo(SelectorArgs),
    /// List done todos
    Done(FilterArgs),
    /// List open todos
    Open(FilterArgs),
}

#[derive(Args)]
pub struct TodoAddArgs {
    /// The task
    #[arg(value_name = "TASK", required = true, num_args = 1..)]
    pub task: Vec<String>,

    /// A comma-separated list of tags
    #[arg(long, value_name = "TAGS", value_delimiter = ',')]
    pub tags: Vec<String>,

    /// Add within the folder at this path
    #[arg(long, value_name = "FOLDER")]
    pub folder: Option<String>,
}

#[derive(Args)]
pub struct TodoListArgs {
    #[command(flatten)]
    pub filters: FilterArgs,

    /// Show done todos as well
    #[arg(long)]
    pub all: bool,
}

#[derive(Args)]
pub struct BookmarkArgs {
    #[command(subcommand)]
    pub command: Option<BookmarkCommand>,

    /// URLs to bookmark
    #[arg(value_name = "URL")]
    pub urls: Vec<String>,

    #[command(flatten)]
    pub opts: BookmarkOpts,
}

#[derive(Args, Default)]
pub struct BookmarkOpts {
    /// A comment or description for this bookmark
    #[arg(short, long, value_name = "COMMENT")]
    pub comment: Option<String>,

    /// A quote or excerpt from the saved page
    #[arg(short, long, visible_alias = "excerpt", value_name = "QUOTE")]
    pub quote: Option<String>,

    /// A comma-separated list of tags
    #[arg(short, long, value_name = "TAGS", value_delimiter = ',')]
    pub tags: Vec<String>,

    /// A URL or selector related to this page; repeat for several
    #[arg(short, long, value_name = "URL|SELECTOR")]
    pub related: Vec<String>,

    /// The filename for the bookmark
    #[arg(short, long, value_name = "FILENAME")]
    pub filename: Option<String>,

    /// The bookmark title (default: the page's own <title>)
    #[arg(long, value_name = "TITLE")]
    pub title: Option<String>,

    /// Add within the folder at this path
    #[arg(long, value_name = "FOLDER")]
    pub folder: Option<String>,

    /// Don't request or download the target page
    #[arg(long)]
    pub no_request: bool,

    /// Save the page source as HTML
    #[arg(long)]
    pub save_source: bool,
}

#[derive(Subcommand)]
pub enum BookmarkCommand {
    /// List bookmarks
    List(FilterArgs),
    /// Print a bookmark's URL
    Url(SelectorArgs),
    /// Open a bookmark's URL in the browser
    Open(SelectorArgs),
    /// View a bookmark in the terminal
    Peek(SelectorArgs),
    /// Edit a bookmark
    Edit(EditArgs),
    /// Delete a bookmark
    Delete(DeleteArgs),
    /// Search bookmarks
    Search(SearchArgs),
}

/// Filters shared by the commands that select sets of notes.
#[derive(Args, Clone, Default)]
pub struct FilterArgs {
    /// Limit to one notebook
    #[arg(short = 'n', long, value_name = "NAME")]
    pub notebook: Option<String>,

    /// Require a tag; repeat to require several
    #[arg(short, long, value_name = "TAG")]
    pub tag: Vec<String>,

    /// Only items touched since a date or duration (`7d`, `3w`, `2026-01-01`)
    #[arg(short, long, value_name = "WHEN")]
    pub since: Option<String>,

    /// Maximum results
    // No short flag: `-l` belongs to `--files-with-matches`, as in grep.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

#[derive(Args)]
pub struct AddArgs {
    /// Filename when it has an extension, otherwise the note's content
    #[arg(value_name = "FILENAME|CONTENT")]
    pub target: Option<String>,

    /// Additional content, when the first argument was a filename
    #[arg(value_name = "CONTENT")]
    pub content_arg: Option<String>,

    /// The content for the new note; `-` reads standard input
    #[arg(short, long, value_name = "CONTENT")]
    pub content: Option<String>,

    /// The filename for the new note
    #[arg(short, long, value_name = "FILENAME")]
    pub filename: Option<String>,

    /// Add within the folder at this path
    #[arg(long, value_name = "FOLDER")]
    pub folder: Option<String>,

    /// A comma-separated list of tags
    #[arg(long, value_name = "TAGS", value_delimiter = ',')]
    pub tags: Vec<String>,

    /// The title for the new note
    #[arg(short, long, value_name = "TITLE")]
    pub title: Option<String>,

    /// The file extension for the new note
    #[arg(long, value_name = "TYPE")]
    pub r#type: Option<String>,

    /// Open the note in the editor even when content was supplied
    #[arg(long)]
    pub edit: bool,

    /// Print the path instead of opening an editor
    #[arg(long)]
    pub no_edit: bool,
}

#[derive(Args)]
pub struct ListArgs {
    /// Notebook, folder, or item to list
    #[arg(value_name = "SELECTOR")]
    pub selector: Option<String>,

    #[command(flatten)]
    pub filters: FilterArgs,

    /// Print only paths
    #[arg(short = 'l', long)]
    pub paths_only: bool,

    /// Print results as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SearchArgs {
    /// Regular expression to search for
    pub pattern: String,

    #[command(flatten)]
    pub filters: FilterArgs,

    /// Match the pattern literally instead of as a regular expression
    #[arg(short = 'F', long)]
    pub fixed_string: bool,

    /// Match case-sensitively (default: smart case)
    #[arg(short = 's', long, conflicts_with = "ignore_case")]
    pub case_sensitive: bool,

    /// Match case-insensitively
    #[arg(short = 'i', long)]
    pub ignore_case: bool,

    /// Matching lines to show per note
    #[arg(short = 'm', long, value_name = "N", default_value_t = kb_core::search::DEFAULT_MAX_MATCHES)]
    pub max_matches: usize,

    /// Print only the paths of matching notes
    #[arg(short = 'l', long)]
    pub files_with_matches: bool,

    /// Print results as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Default)]
pub struct ShowArgs {
    /// Item to show
    #[arg(value_name = "SELECTOR")]
    pub selector: Option<String>,

    #[command(flatten)]
    pub opts: ShowOpts,
}

/// The output-selecting options of `show`, shared with the bare `kb <id>` form.
#[derive(Args, Default)]
pub struct ShowOpts {
    /// Print to standard output instead of paging
    #[arg(short, long)]
    pub print: bool,

    /// Print the full path of the item
    #[arg(long)]
    pub path: bool,

    /// Print the item's path relative to its notebook
    #[arg(long)]
    pub relative_path: bool,

    /// Print the filename of the item
    #[arg(long)]
    pub filename: bool,

    /// Print the id number of the item
    #[arg(long)]
    pub id: bool,

    /// Print the title of the item
    #[arg(long)]
    pub title: bool,

    /// Print the id, filename, and title
    #[arg(long)]
    pub info_line: bool,

    /// Print when the item was added
    #[arg(short, long)]
    pub added: bool,

    /// Print when the item last changed
    #[arg(short, long)]
    pub updated: bool,

    /// Print the file extension of the item
    #[arg(long)]
    pub r#type: bool,
}

#[derive(Args)]
pub struct EditArgs {
    /// Item to edit
    #[arg(value_name = "SELECTOR")]
    pub selector: Option<String>,

    /// Content to add to the item; `-` reads standard input
    #[arg(short, long, value_name = "CONTENT")]
    pub content: Option<String>,

    /// Edit with this editor, overriding $EDITOR
    #[arg(short = 'e', long, value_name = "EDITOR")]
    pub editor: Option<String>,

    /// Edit the last modified item
    #[arg(short = 'l', long)]
    pub last: bool,

    /// Overwrite existing content instead of appending
    #[arg(long)]
    pub overwrite: bool,

    /// Insert content before the existing content
    #[arg(long)]
    pub prepend: bool,
}

#[derive(Args)]
pub struct DeleteArgs {
    /// Items to delete
    #[arg(value_name = "SELECTOR", required = true)]
    pub selectors: Vec<String>,

    /// Skip the confirmation prompt
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Args)]
pub struct MoveArgs {
    /// Item to move
    #[arg(value_name = "SELECTOR")]
    pub selector: String,

    /// Destination notebook, folder, or filename
    #[arg(value_name = "DESTINATION")]
    pub destination: Option<String>,

    /// Rename the item after its title
    #[arg(long, conflicts_with = "destination")]
    pub to_title: bool,

    /// Rename the item to the last modified timestamp
    #[arg(long, conflicts_with = "destination")]
    pub reset: bool,

    /// Skip the confirmation prompt
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Args)]
pub struct CopyArgs {
    /// Item to copy
    #[arg(value_name = "SELECTOR")]
    pub selector: String,

    /// Destination notebook, folder, or filename
    #[arg(value_name = "DESTINATION")]
    pub destination: Option<String>,
}

#[derive(Args)]
pub struct SelectorArgs {
    /// Notebook, folder, or item
    #[arg(value_name = "SELECTOR")]
    pub selector: Option<String>,
}

#[derive(Args)]
pub struct NotebookArgs {
    /// Notebook name
    #[arg(value_name = "NOTEBOOK")]
    pub notebook: Option<String>,
}

#[derive(Args)]
pub struct UseArgs {
    /// Notebook to switch to
    #[arg(value_name = "NOTEBOOK")]
    pub notebook: String,
}

#[derive(Args)]
pub struct GitArgs {
    /// Notebook to run in
    #[arg(short = 'n', long, value_name = "NAME")]
    pub notebook: Option<String>,

    /// Arguments passed through to git
    #[arg(value_name = "ARGS", trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

#[derive(Args)]
pub struct NotebooksArgs {
    #[command(subcommand)]
    pub command: Option<NotebooksCommand>,

    /// Print only notebook names
    #[arg(long)]
    pub names: bool,

    /// Print only notebook paths
    #[arg(long)]
    pub paths: bool,
}

#[derive(Subcommand)]
pub enum NotebooksCommand {
    /// Print the current notebook
    Current(CurrentArgs),
    /// Add a notebook
    Add(NotebookNameArgs),
    /// Delete a notebook
    Delete(DeleteNotebookArgs),
    /// Rename a notebook
    Rename(RenameNotebookArgs),
}

#[derive(Args)]
pub struct CurrentArgs {
    /// Print the notebook's path
    #[arg(long)]
    pub path: bool,
}

#[derive(Args)]
pub struct NotebookNameArgs {
    /// Name of the notebook
    #[arg(value_name = "NAME")]
    pub name: String,
}

#[derive(Args)]
pub struct DeleteNotebookArgs {
    /// Name of the notebook
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Skip the confirmation prompt
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Args)]
pub struct RenameNotebookArgs {
    /// Current name
    #[arg(value_name = "OLD")]
    pub old: String,
    /// New name
    #[arg(value_name = "NEW")]
    pub new: String,
}

#[derive(Args)]
pub struct FoldersArgs {
    #[command(subcommand)]
    pub command: Option<FoldersCommand>,

    /// Notebook or folder to list
    #[arg(value_name = "SELECTOR")]
    pub selector: Option<String>,
}

#[derive(Subcommand)]
pub enum FoldersCommand {
    /// Add a folder
    Add(FolderPathArgs),
    /// Delete a folder
    Delete(FolderPathArgs),
}

#[derive(Args)]
pub struct FolderPathArgs {
    /// Folder path, optionally scoped to a notebook
    #[arg(value_name = "SELECTOR")]
    pub selector: String,

    /// Skip the confirmation prompt
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Args)]
pub struct SyncArgs {
    /// Limit to one notebook
    #[arg(short = 'n', long, value_name = "NAME")]
    pub notebook: Option<String>,

    /// Commit message (default: generated from the staged files)
    #[arg(short, long, value_name = "TEXT")]
    pub message: Option<String>,

    /// Stage every change, not just Markdown
    #[arg(long)]
    pub all: bool,
}

#[derive(Args)]
pub struct InitArgs {
    /// Clone this remote as the initial notebook
    #[arg(value_name = "REMOTE_URL")]
    pub remote: Option<String>,

    /// Branch to clone
    #[arg(value_name = "BRANCH")]
    pub branch: Option<String>,
}

#[derive(Args)]
pub struct PickArgs {
    #[command(flatten)]
    pub filters: FilterArgs,

    /// Initial fzf query
    #[arg(value_name = "QUERY")]
    pub query: Option<String>,

    /// Open the selection in $EDITOR instead of the pager
    #[arg(short, long)]
    pub edit: bool,
}

#[derive(Args)]
pub struct MigrateArgs {
    /// Limit to one notebook
    #[arg(short = 'n', long, value_name = "NAME")]
    pub notebook: Option<String>,

    /// Write the changes; without this the run only reports what it would do
    #[arg(long)]
    pub apply: bool,

    /// Show the value of every key being added
    #[arg(short, long)]
    pub verbose: bool,

    /// Migrate even when a notebook has uncommitted changes
    #[arg(long)]
    pub allow_dirty: bool,
}
