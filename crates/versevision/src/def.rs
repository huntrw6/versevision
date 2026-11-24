//! The command line as clap sees it: the same commands, options, arguments, defaults and help texts as the Python app's typer definitions
//! (`cli.py`, `cli_run.py`, `cli_themes.py`, `cli_explain.py`). The help pages are drawn by `help.rs` from this tree, in typer's layout.
//! `keys` is the only command Python does not have.

use clap::{ArgAction, Args, Parser, Subcommand};

fn int(text: &str) -> Result<i64, String> {
    let t = text.trim();
    t.parse::<i64>()
        .map_err(|_| format!("'{text}' is not a valid int."))
}

#[derive(Parser, Debug)]
#[command(
    name = "versevision",
    about = "Import Bible references into explicitly selected ProPresenter presentations.",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    subcommand_required = false,
    arg_required_else_help = false
)]
pub struct Cli {
    /// Show the version and exit
    #[arg(long, action = ArgAction::SetTrue, help = "Show the version and exit")]
    pub version: bool,
    #[arg(long = "install-completion", action = ArgAction::SetTrue, help = "Install completion for the current shell.")]
    pub install_completion: bool,
    #[arg(long = "show-completion", action = ArgAction::SetTrue, help = "Show completion for the current shell, to copy it or customize the installation.")]
    pub show_completion: bool,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(
        name = "playlists",
        about = "List the ProPresenter playlists and the presentations in each (the same list the web page offers)."
    )]
    Playlists(PlaylistsArgs),
    #[command(
        name = "explain",
        about = "Follow every line of the document to the slides it became: the rule that decided it, how versions were chosen, how verses were split."
    )]
    Explain(ExplainArgs),
    #[command(
        name = "replay",
        about = "Parse a past run's document again with today's rules and list every line whose decision changed (after a rules upgrade)."
    )]
    Replay(ReplayArgs),
    #[command(
        name = "parse",
        about = "Find typed Bible references without retrieving Scripture text."
    )]
    Parse(ParseArgs),
    #[command(
        name = "doctor",
        about = "Check configuration, process state, and Network API connectivity."
    )]
    Doctor(DoctorArgs),
    #[command(
        name = "import",
        about = "Plan or execute an explicitly targeted Scripture-slide import.",
        long_about = "Plan or execute an explicitly targeted Scripture-slide import.\n\nDry runs never write. Production imports require fetched Scripture, a\nvalidated template, an explicit target, and confirmation."
    )]
    Import(ImportArgs),
    #[command(
        name = "presentations",
        about = "List presentations in the output library (default: Verses). Use a name with `run -p NAME --replace/--append`."
    )]
    Presentations(PresentationsArgs),
    #[command(
        name = "run",
        about = "Turn sermon notes into ProPresenter slides (verses and sermon points).",
        long_about = "Turn sermon notes into ProPresenter slides (verses and sermon points).\n\nBy default this creates a NEW presentation named after the document. With -p NAME and --replace\n(swap its verse and point slides) or --append (add to the end), the presentation is replaced under its\nown name: the original is backed up, renamed, replaced by the new version, and removed once ProPresenter\nshows the new one. Playlists keep pointing at it and ProPresenter stays open. --as-copy keeps the\noriginal and saves \"NAME 2\" instead; --in-place edits the file itself (needs a restart)."
    )]
    Run(RunArgs),
    #[command(
        name = "undo",
        about = "Remove the slides a previous import added (and nothing else)."
    )]
    Undo(UndoArgs),
    #[command(
        name = "serve",
        about = "Start the local web page (drag and drop notes, review, import)."
    )]
    Serve(ServeArgs),
    #[command(name = "history", about = "List earlier imports.")]
    History(OutputOnly),
    #[command(
        name = "node",
        about = "Check the connection to the computer that runs ProPresenter."
    )]
    Node(NodeArgs),
    #[command(name = "config", about = "Manage VerseVision configuration.")]
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommand>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(
        name = "themes",
        about = "Use the themes loaded in ProPresenter as the look of verse and point slides.",
        args_conflicts_with_subcommands = false
    )]
    Themes(ThemesArgs),
    #[command(
        name = "propresenter",
        about = "Inspect the configured ProPresenter instance."
    )]
    Propresenter {
        #[command(subcommand)]
        command: Option<PropresenterCommand>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(name = "versions", about = "Inspect known Bible translations.")]
    Versions {
        #[command(subcommand)]
        command: Option<VersionsCommand>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(name = "providers", about = "Inspect configured Bible providers.")]
    Providers {
        #[command(subcommand)]
        command: Option<ProvidersCommand>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(name = "cache", about = "Inspect or clear the Scripture cache.")]
    Cache {
        #[command(subcommand)]
        command: Option<CacheCommand>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(
        name = "template",
        about = "Select and validate a styled ProPresenter Scripture slide."
    )]
    Template {
        #[command(subcommand)]
        command: Option<TemplateCommand>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(
        name = "log",
        about = "Read the record of what VerseVision did (one folder per run).",
        long_about = "Read the record of what VerseVision did (one folder per run)."
    )]
    Log {
        #[command(subcommand)]
        command: Option<LogCommand>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(
        name = "autostart",
        about = "Start the web page automatically when you log in."
    )]
    Autostart {
        #[command(subcommand)]
        command: Option<AutostartCommand>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(
        name = "keys",
        about = "Store, remove or check Bible API keys (never shown)."
    )]
    Keys {
        #[command(subcommand)]
        command: Option<KeysCommand>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
}

#[derive(Args, Debug, Default)]
pub struct OutputOnly {
    #[arg(long, value_name = "str", default_value = "terminal")]
    pub output: String,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Args, Debug)]
pub struct PlaylistsArgs {
    #[arg(
        long,
        value_name = "str",
        help = "Remote computer running ProPresenter"
    )]
    pub node: Option<String>,
    #[arg(long, value_name = "str", default_value = "terminal")]
    pub output: String,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Args, Debug)]
pub struct ExplainArgs {
    #[arg(
        id = "run_id",
        help = "Run id (or the end of it); default: the latest run"
    )]
    pub run_id: Option<String>,
    #[arg(long, short = 'l', value_name = "int", value_parser = int, help = "Only the paragraph that holds this line of the document")]
    pub line: Option<i64>,
    #[arg(
        long,
        value_name = "str",
        help = "Only error, warning or good paragraphs"
    )]
    pub tier: Option<String>,
    #[arg(
        long,
        value_name = "str",
        default_value = "terminal",
        help = "terminal or json"
    )]
    pub output: String,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Args, Debug)]
pub struct ReplayArgs {
    #[arg(id = "run_id", help = "Run id; default: the latest run")]
    pub run_id: Option<String>,
    #[arg(long, short = 'n', value_name = "int", value_parser = int, default_value = "0", help = "Replay this many of the latest runs instead")]
    pub recent: i64,
    #[arg(long = "todays-options", action = ArgAction::SetTrue, help = "Use today's options, not the ones saved with the run")]
    pub todays_options: bool,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Args, Debug)]
pub struct ParseArgs {
    #[arg(id = "file", help = "Document path, or - for stdin")]
    pub file: Option<String>,
    #[arg(long, value_name = "str", help = "Parse text supplied directly")]
    pub text: Option<String>,
    #[arg(
        long,
        value_name = "str",
        default_value = "terminal",
        help = "terminal or json"
    )]
    pub output: String,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    #[arg(long, value_name = "str")]
    pub host: Option<String>,
    #[arg(long, value_name = "int", value_parser = int)]
    pub port: Option<i64>,
    #[arg(long, value_name = "str", default_value = "terminal")]
    pub output: String,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Args, Debug)]
pub struct ImportArgs {
    #[arg(id = "file", help = "Document path, or - for stdin")]
    pub file: Option<String>,
    #[arg(long, value_name = "str")]
    pub text: Option<String>,
    #[arg(long = "default-version", value_name = "str")]
    pub default_version: Option<String>,
    #[arg(long = "target-presentation", value_name = "str")]
    pub target_presentation: Option<String>,
    #[arg(long = "target-uuid", value_name = "str")]
    pub target_uuid: Option<String>,
    #[arg(long, value_name = "str")]
    pub library: Option<String>,
    #[arg(long, value_name = "str")]
    pub host: Option<String>,
    #[arg(long, value_name = "int", value_parser = int)]
    pub port: Option<i64>,
    #[arg(long = "dry-run", action = ArgAction::SetTrue)]
    pub dry_run: bool,
    #[arg(long = "no-fetch", action = ArgAction::SetTrue)]
    pub no_fetch: bool,
    #[arg(long = "non-interactive", action = ArgAction::SetTrue)]
    pub non_interactive: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Confirm the reviewed write transaction")]
    pub yes: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Treat the document as sermon notes: verses plus sermon points")]
    pub notes: bool,
    #[arg(long, value_name = "str", default_value = "terminal")]
    pub output: String,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Args, Debug)]
pub struct PresentationsArgs {
    #[arg(long, value_name = "str")]
    pub node: Option<String>,
    #[arg(long, value_name = "str")]
    pub library: Option<String>,
    #[arg(long = "all", action = ArgAction::SetTrue, help = "Include every library (songs too)")]
    pub all_libraries: bool,
    #[arg(long, value_name = "str", default_value = "terminal")]
    pub output: String,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    #[arg(id = "file", help = "Notes file (.pdf .docx .txt .md .html)")]
    pub file: Option<String>,
    #[arg(long, value_name = "str")]
    pub text: Option<String>,
    #[arg(
        long,
        short = 'p',
        value_name = "str",
        help = "Existing presentation to change instead of making a new one (needs --replace or --append)"
    )]
    pub presentation: Option<String>,
    #[arg(long = "presentation-uuid", value_name = "str")]
    pub presentation_uuid: Option<String>,
    #[arg(
        long,
        value_name = "str",
        help = "Name for the new presentation (default: the document's title)"
    )]
    pub title: Option<String>,
    #[arg(
        long,
        value_name = "str",
        help = "Library for the new presentation (default: Verses)"
    )]
    pub library: Option<String>,
    #[arg(long, action = ArgAction::SetTrue, help = "Copy the chosen presentation without its verse and point slides (blank/background/media stay) and add the new ones")]
    pub replace: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Copy the chosen presentation and add the slides to the end")]
    pub append: bool,
    #[arg(
        long,
        value_name = "str",
        help = "Remote computer running ProPresenter"
    )]
    pub node: Option<String>,
    #[arg(long = "default-version", value_name = "str")]
    pub default_version: Option<String>,
    #[arg(long = "dry-run", action = ArgAction::SetTrue, help = "Plan and report only; change nothing")]
    pub dry_run: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Import even when some lines are flagged")]
    pub yes: bool,
    #[arg(long = "no-playlist", action = ArgAction::SetTrue, help = "Do not add the new file to the playlists that hold the original")]
    pub no_playlist: bool,
    #[arg(long = "as-copy", action = ArgAction::SetTrue, help = "Keep the original and save the result as a numbered copy (Name 2) instead of replacing it")]
    pub as_copy: bool,
    #[arg(long, hide = true, action = ArgAction::SetTrue, help = "(default now; kept so older commands still work)")]
    pub swap: bool,
    #[arg(long = "in-place", action = ArgAction::SetTrue, help = "Edit the original file instead of saving a copy (closes ProPresenter)")]
    pub in_place: bool,
    #[arg(long = "no-close", action = ArgAction::SetTrue, help = "With --in-place: never close ProPresenter automatically")]
    pub no_close: bool,
    #[arg(long = "no-reopen", action = ArgAction::SetTrue)]
    pub no_reopen: bool,
    #[arg(long, value_name = "str", default_value = "terminal")]
    pub output: String,
    #[arg(
        long,
        value_name = "str",
        help = "Use a named set of options from the config ([profiles.NAME.notes] ...)"
    )]
    pub profile: Option<String>,
    #[arg(long = "add-line", value_name = "int", value_parser = int, action = ArgAction::Append, help = "Add a line the importer left out as a slide (repeat for several): the line number shown in the review")]
    pub add_line: Vec<i64>,
    #[arg(long = "set", value_name = "str", action = ArgAction::Append, help = "Change one option for this run: section.option=value (see `versevision config options`)")]
    pub set_options: Vec<String>,
    #[arg(
        long = "target-presentation",
        short = 't',
        hide = true,
        value_name = "str"
    )]
    pub target_presentation: Option<String>,
    #[arg(long = "target-uuid", hide = true, value_name = "str")]
    pub target_uuid: Option<String>,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Args, Debug)]
pub struct UndoArgs {
    #[arg(id = "import_id", help = "Import id from `versevision history`")]
    pub import_id: String,
    #[arg(long, value_name = "str")]
    pub node: Option<String>,
    #[arg(long = "no-close", action = ArgAction::SetTrue)]
    pub no_close: bool,
    #[arg(long, value_name = "str", default_value = "terminal")]
    pub output: String,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Args, Debug)]
pub struct ServeArgs {
    #[arg(
        long,
        value_name = "str",
        default_value = "127.0.0.1",
        help = "Address to listen on; the default keeps it on this computer"
    )]
    pub host: String,
    #[arg(long, value_name = "int", value_parser = int, default_value = "8777")]
    pub port: i64,
    #[arg(long, value_name = "str")]
    pub node: Option<String>,
    #[arg(long = "open", action = ArgAction::SetTrue, overrides_with = "no_open")]
    pub open: bool,
    #[arg(long = "no-open", action = ArgAction::SetTrue, overrides_with = "open")]
    pub no_open: bool,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Args, Debug)]
pub struct NodeArgs {
    #[arg(long, value_name = "str")]
    pub node: Option<String>,
    #[arg(long, value_name = "str", default_value = "terminal")]
    pub output: String,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    #[command(name = "init")]
    Init {
        #[arg(long, action = ArgAction::SetTrue, help = "Replace an existing configuration")]
        force: bool,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(name = "show")]
    Show(OutputOnly),
    #[command(
        name = "options",
        about = "List every option for reading the notes and formatting slides: its name, choices and current value."
    )]
    Options {
        #[arg(
            id = "section",
            help = "notes, points, verses or slides (default: all)"
        )]
        section: Option<String>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(name = "set", about = "Change one option in the saved configuration.")]
    Set {
        #[arg(
            id = "key",
            required = true,
            help = "section.option, for example points.bullet_char"
        )]
        key: String,
        #[arg(
            id = "value",
            required = true,
            help = "the new value (a list is written as JSON: '[\"pause\"]')"
        )]
        value: String,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
}

#[derive(Args, Debug)]
pub struct ThemesArgs {
    #[arg(
        long,
        value_name = "str",
        help = "Remote computer running ProPresenter"
    )]
    pub node: Option<String>,
    #[arg(long, value_name = "str", default_value = "terminal")]
    pub output: String,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
    #[command(subcommand)]
    pub command: Option<ThemesCommand>,
}

#[derive(Subcommand, Debug)]
pub enum ThemesCommand {
    #[command(
        name = "use",
        about = "Save the theme choice. It is checked against the themes loaded on the Mac before it is saved."
    )]
    Use {
        #[arg(
            long,
            value_name = "str",
            help = "Theme for verse slides (Name or Group/Name)"
        )]
        verses: Option<String>,
        #[arg(
            long = "verse-slide",
            value_name = "str",
            default_value = "",
            help = "Which slide of it: name or number (default: the first with a text box)"
        )]
        verse_slide: String,
        #[arg(long, value_name = "str", help = "Theme for sermon-point slides")]
        points: Option<String>,
        #[arg(long = "point-slide", value_name = "str", default_value = "")]
        point_slide: String,
        #[arg(long, action = ArgAction::SetTrue, help = "Go back to the VerseVision template slides")]
        clear: bool,
        #[arg(long, value_name = "str")]
        node: Option<String>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PropresenterCommand {
    #[command(name = "libraries")]
    Libraries(OutputOnly),
    #[command(name = "presentations")]
    Presentations(OutputOnly),
    #[command(name = "playlists")]
    Playlists(OutputOnly),
}

#[derive(Subcommand, Debug)]
pub enum VersionsCommand {
    #[command(name = "list")]
    List(OutputOnly),
}

#[derive(Subcommand, Debug)]
pub enum ProvidersCommand {
    #[command(name = "test")]
    Test(OutputOnly),
}

#[derive(Subcommand, Debug)]
pub enum CacheCommand {
    #[command(name = "inspect")]
    Inspect(OutputOnly),
    #[command(name = "clear")]
    Clear {
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
}

#[derive(Subcommand, Debug)]
pub enum TemplateCommand {
    #[command(name = "inspect")]
    Inspect {
        #[arg(long = "target-uuid", value_name = "str")]
        target_uuid: Option<String>,
        #[arg(long = "target-presentation", value_name = "str")]
        target_presentation: Option<String>,
        #[arg(long, value_name = "str")]
        library: Option<String>,
        #[arg(long, value_name = "str")]
        host: Option<String>,
        #[arg(long, value_name = "int", value_parser = int)]
        port: Option<i64>,
        #[arg(long, value_name = "str", default_value = "terminal")]
        output: String,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(name = "select")]
    Select {
        #[arg(long = "target-uuid", value_name = "str")]
        target_uuid: Option<String>,
        #[arg(long = "target-presentation", value_name = "str")]
        target_presentation: Option<String>,
        #[arg(long, value_name = "str")]
        library: Option<String>,
        #[arg(long = "cue-uuid", value_name = "str")]
        cue_uuid: Option<String>,
        #[arg(long, value_name = "str")]
        host: Option<String>,
        #[arg(long, value_name = "int", value_parser = int)]
        port: Option<i64>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(
        name = "select-point",
        about = "Choose the slide used for sermon points (a slide with one text object named 'Sermon Point')."
    )]
    SelectPoint {
        #[arg(
            long = "cue-uuid",
            value_name = "str",
            help = "Cue holding the sermon-point template slide"
        )]
        cue_uuid: Option<String>,
        #[arg(
            long = "object",
            value_name = "str",
            default_value = "Sermon Point",
            help = "Name of the point text object"
        )]
        point_object: String,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(name = "validate")]
    Validate(OutputOnly),
    #[command(
        name = "check",
        about = "Check that the saved verse and sermon-point templates still exist and look the same (works through a node)."
    )]
    Check(NodeArgs),
    #[command(
        name = "new-file",
        about = "Move the template slides into their own presentation, so a sermon edit can never delete them."
    )]
    NewFile {
        #[arg(
            long = "source-file",
            value_name = "str",
            help = "A presentation FILE that still holds the template slides (a backup works)"
        )]
        source_file: Option<String>,
        #[arg(
            long,
            value_name = "str",
            help = "Name of a presentation in ProPresenter that holds them"
        )]
        source: Option<String>,
        #[arg(long, value_name = "str", default_value = "VerseVision Template")]
        name: String,
        #[arg(
            long,
            value_name = "str",
            help = "Library for the template file (default: the output library)"
        )]
        library: Option<String>,
        #[arg(long, value_name = "str")]
        node: Option<String>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(
        name = "setup",
        about = "Pick the verse and sermon-point template slides (works through a remote node)."
    )]
    Setup {
        #[arg(
            long = "target-presentation",
            short = 't',
            value_name = "str",
            default_value = "Verses"
        )]
        target_presentation: String,
        #[arg(long, value_name = "str")]
        library: Option<String>,
        #[arg(long = "verse-slide", value_name = "int", value_parser = int, help = "Slide number holding the verse template")]
        verse_slide: Option<i64>,
        #[arg(long = "point-slide", value_name = "int", value_parser = int, help = "Slide number holding the sermon-point template")]
        point_slide: Option<i64>,
        #[arg(long, value_name = "str")]
        node: Option<String>,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
}

#[derive(Args, Debug)]
pub struct LogShowArgs {
    #[arg(id = "run_id", help = "Run id (or the end of it); default: the latest")]
    pub run_id: Option<String>,
    #[arg(long, action = ArgAction::SetTrue, help = "Every logged step instead of the summary")]
    pub events: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Only warnings and errors")]
    pub problems: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Print the folder path only")]
    pub path: bool,
    #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
    pub help: Option<bool>,
}

#[derive(Subcommand, Debug)]
pub enum LogCommand {
    #[command(
        name = "show",
        about = "Show one run: the readable summary, or --events / --problems."
    )]
    Show(LogShowArgs),
    #[command(name = "list", about = "List recent runs, newest first.")]
    List {
        #[arg(long, short = 'n', value_name = "int", value_parser = int, default_value = "20")]
        count: i64,
        #[arg(long, value_name = "str", default_value = "terminal")]
        output: String,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
}

#[derive(Subcommand, Debug)]
pub enum AutostartCommand {
    #[command(name = "status")]
    Status(OutputOnly),
    #[command(
        name = "enable",
        about = "Turn on: the web page starts when you log in and restarts if it stops."
    )]
    Enable {
        #[arg(long, value_name = "str", default_value = "127.0.0.1")]
        host: String,
        #[arg(long, value_name = "int", value_parser = int, default_value = "8777")]
        port: i64,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(name = "disable", about = "Turn off and remove the login item.")]
    Disable {
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
}

#[derive(Subcommand, Debug)]
pub enum KeysCommand {
    #[command(
        name = "set",
        about = "Store a key. It is read from a hidden prompt, or from standard input when that is not a terminal; never from an argument."
    )]
    Set {
        #[arg(
            id = "provider",
            required = true,
            help = "api_bible, youversion or esv"
        )]
        provider: String,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(
        name = "remove",
        about = "Forget a stored key (an environment variable is not touched)."
    )]
    Remove {
        #[arg(
            id = "provider",
            required = true,
            help = "api_bible, youversion or esv"
        )]
        provider: String,
        #[arg(long, action = ArgAction::Help, help = "Show this message and exit.")]
        help: Option<bool>,
    },
    #[command(
        name = "status",
        about = "Say for each provider whether a key is set. A key is never shown."
    )]
    Status(OutputOnly),
}
