//! The rules engine: `rules.d/*.json` files evaluated per process, one stage
//! at a time. Built-ins are embedded at build time (`build.rs`); user and
//! `/etc` files add rules and `disable` others, never replace a file by name.
//!
//! `Facts` borrows the `Process`; every test folds ASCII case on the fly, so
//! evaluation allocates nothing and a slice that lands inside a multibyte
//! character is a miss rather than a panic.
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};

include!(concat!(env!("OUT_DIR"), "/builtin_rules.rs"));

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Unit,
    Class,
    Session,
    App,
    Placement,
}
pub const STAGES: [Stage; 5] = [
    Stage::Unit,
    Stage::Class,
    Stage::Session,
    Stage::App,
    Stage::Placement,
];

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct RuleFile {
    pub stage: Stage,
    #[serde(default)]
    pub disable: Vec<String>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub examples: Vec<Example>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    #[serde(rename = "match")]
    pub test: Match,
    #[serde(default)]
    pub identity: Option<String>,
    #[serde(default)]
    pub folder: Option<FolderName>,
    #[serde(default)]
    pub fold_to: Option<String>,
    #[serde(default)]
    pub owner_uid: Option<u32>,
    #[serde(default)]
    pub classes: Vec<Class>,
    #[serde(default)]
    pub flags: Vec<UnitFlag>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum FolderName {
    Applications,
    UserServices,
}

impl From<FolderName> for crate::types::Folder {
    fn from(f: FolderName) -> Self {
        match f {
            FolderName::Applications => Self::Applications,
            FolderName::UserServices => Self::UserServices,
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum Match {
    Name(Pat),
    NamePrefix(Pat),
    NameSuffix(Pat),
    NameContains(Pat),
    ExePrefix(Pat),
    ExeSuffix(Pat),
    ExeContains(Pat),
    ArgPrefix(Pat),
    ArgContains(Pat),
    CgroupContains(Pat),
    Unit(Pat),
    UnitPrefix(Pat),
    UnitSuffix(Pat),
    UnitContains(Pat),
    Script(Pat),
    Identity(Pat),
    Container(Pat),
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum Pat {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Launcher,
    Generic,
    Shell,
    Terminal,
    Compositor,
    Worker,
    Noise,
    CrashHelper,
    NoAbsorb,
    AnonymousScript,
    ContainerRuntime,
}
pub const CLASS_NAMES: [&str; 11] = [
    "launcher",
    "generic",
    "shell",
    "terminal",
    "compositor",
    "worker",
    "noise",
    "crash_helper",
    "no_absorb",
    "anonymous_script",
    "container_runtime",
];

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum UnitFlag {
    Lying,
    Service,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Classes(pub u16);
impl Classes {
    pub const LAUNCHER: u16 = 1 << 0;
    pub const GENERIC: u16 = 1 << 1;
    pub const SHELL: u16 = 1 << 2;
    pub const TERMINAL: u16 = 1 << 3;
    pub const COMPOSITOR: u16 = 1 << 4;
    pub const WORKER: u16 = 1 << 5;
    pub const NOISE: u16 = 1 << 6;
    pub const CRASH_HELPER: u16 = 1 << 7;
    pub const NO_ABSORB: u16 = 1 << 8;
    pub const ANONYMOUS_SCRIPT: u16 = 1 << 9;
    pub const CONTAINER_RUNTIME: u16 = 1 << 10;
    #[must_use]
    pub const fn bit(c: Class) -> u16 {
        1 << (c as u16)
    }
    #[must_use]
    pub const fn has(self, bits: u16) -> bool {
        self.0 & bits != 0
    }
    #[must_use]
    pub const fn contains(self, c: Class) -> bool {
        self.has(Self::bit(c))
    }
    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        (0..CLASS_NAMES.len())
            .filter(|i| self.0 & (1 << i) != 0)
            .map(|i| CLASS_NAMES[i])
            .collect()
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct UnitFlags(pub u8);
impl UnitFlags {
    pub const LYING: u8 = 1;
    pub const SERVICE: u8 = 2;
    #[must_use]
    pub const fn lying(self) -> bool {
        self.0 & Self::LYING != 0
    }
    #[must_use]
    pub const fn service(self) -> bool {
        self.0 & Self::SERVICE != 0
    }
    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        let mut v = vec![];
        if self.lying() {
            v.push("lying");
        }
        if self.service() {
            v.push("service");
        }
        v
    }
}

/// Example: the flat `--fixture` row plus the unit/identity/container subjects.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Example {
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub ppid: u32,
    #[serde(default)]
    pub pgrp: i32,
    #[serde(default)]
    pub uid: u32,
    #[serde(default)]
    pub kthread: bool,
    #[serde(default)]
    pub utime: u64,
    #[serde(default)]
    pub stime: u64,
    #[serde(default)]
    pub comm: Option<String>,
    #[serde(default)]
    pub exe: Option<String>,
    #[serde(default)]
    pub cmdline: Vec<String>,
    #[serde(default)]
    pub cgroup: String,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub identity: Option<String>,
    #[serde(default)]
    pub container: Option<String>,
    /// `deserialize_with` and no `default` makes the key required, so a
    /// forgotten `expect` fails the file instead of reading as "no match".
    #[serde(deserialize_with = "Option::deserialize")]
    pub expect: Option<Expect>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub rule: Option<String>,
    pub rules: Option<Vec<String>>,
    pub identity: Option<String>,
    pub folder: Option<FolderName>,
    pub classes: Option<Vec<Class>>,
    pub flags: Option<Vec<UnitFlag>>,
    pub fold_to: Option<String>,
    pub owner_uid: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Name,
    Exe,
    Arg,
    Cgroup,
    Unit,
    Script,
    Identity,
    Container,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Prefix,
    Suffix,
    Contains,
}
#[derive(Debug)]
enum Test {
    Str(Field, Op, Vec<String>),
    /// Equality with patterns bucketed by byte length, index = length, so a
    /// haystack skips every pattern of another length.
    Eq(Field, Vec<Vec<String>>),
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
}

#[derive(Debug)]
pub struct Compiled {
    pub source: Source,
    pub file: String,
    pub id: String,
    test: Test,
    pub identity: Option<String>,
    pub folder: Option<FolderName>,
    pub fold_to: Option<String>,
    pub owner_uid: Option<u32>,
    pub classes: Classes,
    pub flags: UnitFlags,
}
impl Compiled {
    #[must_use]
    pub fn name(&self) -> String {
        format!("{}:{}", self.file, self.id)
    }
    /// What the rule decides, for `--explain`: `tdeinit user_services`, `shell`.
    #[must_use]
    pub fn outputs(&self) -> String {
        let mut v: Vec<String> = self.identity.iter().cloned().collect();
        v.extend(self.fold_to.iter().map(|k| format!("fold_to {k}")));
        v.extend(self.folder.map(|f| folder_name(f).to_string()));
        v.extend(self.owner_uid.map(|u| format!("owner_uid {u}")));
        v.extend(self.classes.names().into_iter().map(str::to_string));
        v.extend(self.flags.names().into_iter().map(str::to_string));
        v.join(" ")
    }
}

/// Where a file came from. Lower `rank` wins: `HEFT_RULES_PATH` index, or 0
/// for XDG and 1 for `/etc`; built-ins are `usize::MAX`.
#[derive(Clone, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub struct Source {
    pub rank: usize,
    pub label: String,
}
impl Source {
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            rank: usize::MAX,
            label: "built-in".into(),
        }
    }
}

pub struct LoadedFile {
    pub source: Source,
    pub name: String,
    pub text: String,
}

/// What one process looks like to the engine. Borrowed, never lowercased.
#[derive(Default, Clone, Copy)]
pub struct Facts<'a> {
    pub comm: &'a str,
    pub name: &'a str,
    pub exe: Option<&'a str>,
    pub argv: &'a [String],
    pub cgroup: &'a str,
    pub unit: Option<&'a str>,
    pub script: Option<&'a str>,
    pub identity: Option<&'a str>,
    pub container: Option<&'a str>,
}

#[derive(Debug)]
pub struct Problem {
    pub source: Source,
    pub file: String,
    pub what: String,
}

/// One parsed-and-compiled file, before `disable`.
struct FileUnit {
    source: Source,
    name: String,
    stage: Stage,
    disable: Vec<String>,
    rules: Vec<Compiled>,
    examples: Vec<Example>,
}

pub struct Rules {
    stages: BTreeMap<Stage, Vec<Compiled>>,
    pub problems: Vec<Problem>,
    pub warnings: Vec<String>,
    /// Files that compiled, with their examples, for `check`.
    files: Vec<(Source, String, Stage, Vec<Example>)>,
    /// Every `disable` entry, with the file and source label that carried it.
    disabled: BTreeMap<String, (String, String)>,
}

/// What `--check-rules` found: the lines it prints and the summary counts.
pub struct Report {
    pub lines: Vec<String>,
    pub examples: usize,
    pub files: usize,
    pub failed: usize,
    pub overridden: usize,
    pub disabled: usize,
}

/// The embedded `rules.d` as loader input.
pub fn builtin_files() -> impl Iterator<Item = LoadedFile> {
    BUILTIN.iter().map(|(name, text)| LoadedFile {
        source: Source::builtin(),
        name: (*name).to_string(),
        text: (*text).to_string(),
    })
}

fn example_name(file: &str, ex: &Example, i: usize, source: &Source) -> String {
    match ex.expect.as_ref().and_then(|e| e.rule.as_deref()) {
        Some(rule) => format!("{file}:{rule}#{i} ({})", source.label),
        None => format!("{file}#{i} ({})", source.label),
    }
}

/// `--check-rules`: every loaded file's examples, one line per failure or
/// difference, then a summary. `Ok(false)` means exit 1.
///
/// # Errors
///
/// Returns an error if stdout cannot be written.
pub fn print_check() -> Result<bool, crate::types::Error> {
    use std::io::Write;
    let r = Rules::load().report(&Rules::builtin());
    let mut out = std::io::stdout().lock();
    for l in &r.lines {
        writeln!(out, "{l}")?;
    }
    writeln!(
        out,
        "{} examples in {} files: {} failed, {} overridden, {} disabled",
        r.examples, r.files, r.failed, r.overridden, r.disabled
    )?;
    Ok(r.failed == 0)
}

fn id_ok(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `<file>.json` or `<file>.json:<id>`: a non-empty base name with no `/` or
/// `:`, and an id inside the id grammar.
fn disable_ok(e: &str) -> bool {
    let (file, id) = match e.split_once(':') {
        Some((f, i)) => (f, Some(i)),
        None => (e, None),
    };
    let stem = file.strip_suffix(".json");
    stem.is_some_and(|s| !s.is_empty() && !s.contains('/') && !s.contains(':'))
        && id.is_none_or(id_ok)
}

fn pat(p: Pat) -> Result<Vec<String>, String> {
    let v = match p {
        Pat::One(s) => vec![s],
        Pat::Many(v) => v,
    };
    if v.is_empty() {
        return Err("empty pattern list".into());
    }
    if v.iter().any(String::is_empty) {
        return Err("empty string pattern".into());
    }
    Ok(v.into_iter().map(|s| s.to_ascii_lowercase()).collect())
}

fn bucket(v: Vec<String>) -> Vec<Vec<String>> {
    let max = v.iter().map(String::len).max().unwrap_or(0);
    let mut out: Vec<Vec<String>> = (0..=max).map(|_| vec![]).collect();
    for p in v {
        let l = p.len();
        out[l].push(p);
    }
    out
}

#[derive(Default)]
struct CompileCtx {
    uses_script: bool,
    uses_placement_key: bool,
}

fn compile_match(m: Match, ctx: &mut CompileCtx) -> Result<Test, String> {
    use Field::{Arg, Cgroup, Container, Exe, Identity, Name, Script, Unit};
    use Op::{Contains, Prefix, Suffix};
    Ok(match m {
        Match::Name(p) => Test::Eq(Name, bucket(pat(p)?)),
        Match::NamePrefix(p) => Test::Str(Name, Prefix, pat(p)?),
        Match::NameSuffix(p) => Test::Str(Name, Suffix, pat(p)?),
        Match::NameContains(p) => Test::Str(Name, Contains, pat(p)?),
        Match::ExePrefix(p) => Test::Str(Exe, Prefix, pat(p)?),
        Match::ExeSuffix(p) => Test::Str(Exe, Suffix, pat(p)?),
        Match::ExeContains(p) => Test::Str(Exe, Contains, pat(p)?),
        Match::ArgPrefix(p) => Test::Str(Arg, Prefix, pat(p)?),
        Match::ArgContains(p) => Test::Str(Arg, Contains, pat(p)?),
        Match::CgroupContains(p) => Test::Str(Cgroup, Contains, pat(p)?),
        Match::Unit(p) => Test::Eq(Unit, bucket(pat(p)?)),
        Match::UnitPrefix(p) => Test::Str(Unit, Prefix, pat(p)?),
        Match::UnitSuffix(p) => Test::Str(Unit, Suffix, pat(p)?),
        Match::UnitContains(p) => Test::Str(Unit, Contains, pat(p)?),
        Match::Script(p) => {
            ctx.uses_script = true;
            Test::Eq(Script, bucket(pat(p)?))
        }
        Match::Identity(p) => {
            ctx.uses_placement_key = true;
            Test::Eq(Identity, bucket(pat(p)?))
        }
        Match::Container(p) => {
            ctx.uses_placement_key = true;
            Test::Eq(Container, bucket(pat(p)?))
        }
        Match::All(v) => {
            if v.is_empty() {
                return Err("empty all".into());
            }
            Test::All(
                v.into_iter()
                    .map(|m| compile_match(m, ctx))
                    .collect::<Result<_, _>>()?,
            )
        }
        Match::Any(v) => {
            if v.is_empty() {
                return Err("empty any".into());
            }
            Test::Any(
                v.into_iter()
                    .map(|m| compile_match(m, ctx))
                    .collect::<Result<_, _>>()?,
            )
        }
        Match::Not(b) => Test::Not(Box::new(compile_match(*b, ctx)?)),
    })
}

fn compile_rule(stage: Stage, r: Rule, file: &str, source: &Source) -> Result<Compiled, String> {
    let mut ctx = CompileCtx::default();
    let test = compile_match(r.test, &mut ctx).map_err(|e| format!("rule {}: {e}", r.id))?;
    let outputs: Vec<&str> = [
        (r.identity.is_some(), "identity"),
        (r.folder.is_some(), "folder"),
        (r.fold_to.is_some(), "fold_to"),
        (r.owner_uid.is_some(), "owner_uid"),
        (!r.classes.is_empty(), "classes"),
        (!r.flags.is_empty(), "flags"),
    ]
    .into_iter()
    .filter_map(|(on, what)| on.then_some(what))
    .collect();
    let (required, optional): (&[&str], &[&str]) = match stage {
        Stage::Unit => (&["flags"], &[]),
        Stage::Class => (&["classes"], &[]),
        Stage::Session => (&["identity", "folder"], &[]),
        Stage::App => (&["identity"], &[]),
        Stage::Placement => (&[], &["fold_to", "folder", "owner_uid"]),
    };
    for o in &outputs {
        if !required.contains(o) && !optional.contains(o) {
            return Err(format!(
                "rule {}: `{o}` is not an output of stage {stage:?}",
                r.id
            ));
        }
    }
    for req in required {
        if !outputs.contains(req) {
            return Err(format!("rule {}: stage {stage:?} requires `{req}`", r.id));
        }
    }
    if stage == Stage::Placement {
        if outputs.is_empty() {
            return Err(format!(
                "rule {}: placement needs fold_to, folder or owner_uid",
                r.id
            ));
        }
        if r.owner_uid.is_some() && (r.fold_to.is_some() || r.folder.is_some()) {
            return Err(format!(
                "rule {}: owner_uid cannot be combined with fold_to or folder",
                r.id
            ));
        }
    }
    if ctx.uses_script && !(stage == Stage::Class && r.classes == [Class::AnonymousScript]) {
        return Err(format!(
            "rule {}: `script` only in a class rule whose classes is [anonymous_script]",
            r.id
        ));
    }
    if ctx.uses_placement_key && stage != Stage::Placement {
        return Err(format!(
            "rule {}: `identity`/`container` tests are placement-only",
            r.id
        ));
    }
    let mut classes = Classes::default();
    for c in &r.classes {
        classes.0 |= Classes::bit(*c);
    }
    let mut flags = UnitFlags::default();
    for f in &r.flags {
        flags.0 |= match f {
            UnitFlag::Lying => UnitFlags::LYING,
            UnitFlag::Service => UnitFlags::SERVICE,
        };
    }
    Ok(Compiled {
        source: source.clone(),
        file: file.to_string(),
        id: r.id,
        test,
        identity: r.identity,
        folder: r.folder,
        fold_to: r.fold_to,
        owner_uid: r.owner_uid,
        classes,
        flags,
    })
}

fn compile_file(f: &LoadedFile) -> Result<FileUnit, String> {
    let text = crate::config::strip_comments(&f.text);
    let rf: RuleFile = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    let mut rules = Vec::with_capacity(rf.rules.len());
    for r in rf.rules {
        if !id_ok(&r.id) {
            return Err(format!("id `{}` outside [a-z0-9-]+", r.id));
        }
        if !seen.insert(r.id.clone()) {
            return Err(format!("duplicate id `{}`", r.id));
        }
        rules.push(compile_rule(rf.stage, r, &f.name, &f.source)?);
    }
    for d in &rf.disable {
        if !disable_ok(d) {
            return Err(format!(
                "disable entry `{d}` is not <file>.json or <file>.json:<id>"
            ));
        }
    }
    for (i, e) in rf.examples.iter().enumerate() {
        if e.comm.is_none() && e.unit.is_none() && e.identity.is_none() && e.container.is_none() {
            return Err(format!("example #{i} has no subject"));
        }
    }
    Ok(FileUnit {
        source: f.source.clone(),
        name: f.name.clone(),
        stage: rf.stage,
        disable: rf.disable,
        rules,
        examples: rf.examples,
    })
}

impl Rules {
    /// The rules every sampler and `--explain` share, resolved once the way
    /// `glyph` and `root` are. Warnings print inside the init, so once per run.
    pub fn load() -> &'static Self {
        static RULES: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
        RULES.get_or_init(|| {
            let legacy = crate::config::config_dir().join("grouping.json");
            if legacy.exists() {
                eprintln!(
                    "heft: {} is no longer read; rules.d replaced it, see HUMANS.md",
                    legacy.display()
                );
            }
            let mut warnings = vec![];
            let mut files = vec![];
            // `HEFT_RULES_PATH` set, even empty, replaces both directories.
            if let Some(list) = std::env::var_os("HEFT_RULES_PATH") {
                use std::os::unix::ffi::OsStrExt;
                for (rank, dir) in list.as_bytes().split(|&b| b == b':').enumerate() {
                    if dir.is_empty() {
                        continue;
                    }
                    let path = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(dir));
                    let source = Source {
                        rank,
                        label: path.display().to_string(),
                    };
                    files.extend(load_dir(&path, &source, &mut warnings));
                }
            } else {
                for (rank, path) in [
                    crate::config::config_dir().join("rules.d"),
                    std::path::PathBuf::from("/etc/heft/rules.d"),
                ]
                .into_iter()
                .enumerate()
                {
                    let source = Source {
                        rank,
                        label: path.display().to_string(),
                    };
                    files.extend(load_dir(&path, &source, &mut warnings));
                }
            }
            files.extend(builtin_files());
            let mut r = Self::from_files(files);
            r.warnings.splice(0..0, warnings);
            for p in &r.problems {
                eprintln!("heft: skipping {} ({}): {}", p.file, p.source.label, p.what);
            }
            for w in &r.warnings {
                eprintln!("heft: {w}");
            }
            r
        })
    }

    /// The embedded `rules.d`. Panics on a malformed file, which is safe only
    /// because `tests::builtin_files_parse_and_their_examples_pass` parses the
    /// same table under `cargo test`.
    #[must_use]
    pub fn builtin() -> Self {
        let r = Self::from_files(builtin_files().collect());
        assert!(r.problems.is_empty(), "built-in rules: {:?}", r.problems);
        r
    }

    #[must_use]
    pub fn from_files(files: Vec<LoadedFile>) -> Self {
        let mut problems = vec![];
        let mut units: Vec<FileUnit> = vec![];
        for f in &files {
            match compile_file(f) {
                Ok(u) => units.push(u),
                Err(what) => problems.push(Problem {
                    source: f.source.clone(),
                    file: f.name.clone(),
                    what,
                }),
            }
        }
        // Source precedence, then bytewise file name.
        units.sort_by(|a, b| {
            (&a.source.rank, a.name.as_bytes()).cmp(&(&b.source.rank, b.name.as_bytes()))
        });
        // A file that failed to compile is skipped whole, `disable` included.
        let mut disabled: BTreeMap<String, (String, String)> = BTreeMap::new();
        for u in &units {
            for d in &u.disable {
                disabled
                    .entry(d.clone())
                    .or_insert_with(|| (u.name.clone(), u.source.label.clone()));
            }
        }
        let mut warnings = vec![];
        for d in disabled.keys() {
            let names_something = units.iter().any(|u| match d.split_once(':') {
                None => u.name == *d,
                Some((f, id)) => u.name == f && u.rules.iter().any(|r| r.id == id),
            });
            if !names_something {
                warnings.push(format!("disable `{d}` names no loaded file or rule"));
            }
        }
        let mut stages: BTreeMap<Stage, Vec<Compiled>> =
            STAGES.iter().map(|s| (*s, vec![])).collect();
        let mut kept_files = vec![];
        for u in units {
            let rules: Vec<Compiled> = u
                .rules
                .into_iter()
                .filter(|r| !disabled.contains_key(&r.file) && !disabled.contains_key(&r.name()))
                .collect();
            stages
                .get_mut(&u.stage)
                .expect("every stage is seeded")
                .extend(rules);
            kept_files.push((u.source, u.name, u.stage, u.examples));
        }
        Self {
            stages,
            problems,
            warnings,
            files: kept_files,
            disabled,
        }
    }

    #[must_use]
    pub fn is_empty(&self, stage: Stage) -> bool {
        self.stages[&stage].is_empty()
    }
    #[must_use]
    pub fn count(&self, stage: Stage) -> usize {
        self.stages[&stage].len()
    }

    #[inline]
    #[must_use]
    pub fn unit_flags(&self, unit: &str) -> UnitFlags {
        let f = Facts {
            unit: Some(unit),
            ..Facts::default()
        };
        let mut out = UnitFlags::default();
        for r in &self.stages[&Stage::Unit] {
            if eval(&r.test, &f) {
                out.0 |= r.flags.0;
            }
        }
        out
    }
    #[inline]
    #[must_use]
    pub fn classes(&self, f: &Facts) -> Classes {
        let mut out = Classes::default();
        for r in &self.stages[&Stage::Class] {
            if eval(&r.test, f) {
                out.0 |= r.classes.0;
            }
        }
        out
    }
    /// Class lookup on a bare name (an identity, a path basename): `comm` and
    /// `name` are the string and every other fact is empty, so a class rule
    /// with an `exe_` or `arg_` test never fires here.
    #[must_use]
    pub fn classes_of_name(&self, name: &str) -> Classes {
        self.classes(&Facts {
            comm: name,
            name,
            ..Facts::default()
        })
    }
    #[inline]
    #[must_use]
    pub fn session(&self, f: &Facts) -> Option<(&str, FolderName)> {
        self.stages[&Stage::Session]
            .iter()
            .find(|r| eval(&r.test, f))
            .map(|r| {
                (
                    r.identity.as_deref().expect("session rules carry identity"),
                    r.folder.expect("session rules carry folder"),
                )
            })
    }
    #[inline]
    #[must_use]
    pub fn app(&self, f: &Facts) -> Option<&str> {
        self.stages[&Stage::App]
            .iter()
            .find(|r| eval(&r.test, f))
            .and_then(|r| r.identity.as_deref())
    }
    #[must_use]
    pub fn placement(&self, identity: &str) -> Option<&Compiled> {
        let f = Facts {
            identity: Some(identity),
            ..Facts::default()
        };
        self.deciding(Stage::Placement, &f).first().copied()
    }
    #[must_use]
    pub fn container_owner(&self, name: &str) -> Option<u32> {
        let f = Facts {
            container: Some(name),
            ..Facts::default()
        };
        self.deciding(Stage::Placement, &f)
            .first()
            .and_then(|r| r.owner_uid)
    }

    /// Every rule of the stage that matches, in order (flag stages) or the
    /// first (identity stages): what `--explain` and `check` report.
    ///
    /// Placement is two lists by output kind: a container subject sees only
    /// `owner_uid` rules and an identity only `fold_to`/`folder` rules. In one
    /// list `{"not": {"container": "x"}}` holds for every identity, which has
    /// no container, and shadowed every pin listed after it.
    #[must_use]
    pub fn deciding<'r>(&'r self, stage: Stage, f: &Facts) -> Vec<&'r Compiled> {
        let it = self.stages[&stage]
            .iter()
            .filter(|r| r.owner_uid.is_some() == f.container.is_some() && eval(&r.test, f));
        match stage {
            Stage::Unit | Stage::Class => it.collect(),
            Stage::Session | Stage::App | Stage::Placement => it.take(1).collect(),
        }
    }

    #[must_use]
    pub fn examples_total(&self) -> usize {
        self.files.iter().map(|f| f.3.len()).sum()
    }

    /// Every loaded file's examples against this set, one line per failure.
    /// `cargo test` holds the built-in set to an empty result.
    #[must_use]
    pub fn check(&self) -> Vec<String> {
        let mut out = vec![];
        for (source, file, stage, examples) in &self.files {
            for (i, ex) in examples.iter().enumerate() {
                let owned = subject_facts(ex, *stage);
                let fails = judge(ex, &self.deciding(*stage, &owned.facts()), file);
                if !fails.is_empty() {
                    let name = example_name(file, ex, i, source);
                    out.push(format!("{name}: {}", fails.join("; ")));
                }
            }
        }
        out
    }

    /// `--check-rules` over this merged set. A built-in example is judged
    /// against `builtin` alone, where a failure is a bug in the binary, and
    /// against this set only to say what a user file changed: `overridden by`
    /// when a higher source decided it, `disabled by` when a `disable` removed
    /// the rule. Neither fails: judged against the merged set alone, one
    /// `disable` of `40-trinity.json` failed 14 of that file's 18 examples.
    #[must_use]
    pub fn report(&self, builtin: &Self) -> Report {
        let mut r = Report {
            lines: vec![],
            examples: 0,
            files: self.files.len() + self.problems.len(),
            failed: 0,
            overridden: 0,
            disabled: 0,
        };
        for p in &self.problems {
            r.failed += 1;
            r.lines
                .push(format!("{} ({}): {}", p.file, p.source.label, p.what));
        }
        for (source, file, stage, examples) in &self.files {
            let is_builtin = source.rank == usize::MAX;
            for (i, ex) in examples.iter().enumerate() {
                r.examples += 1;
                let name = example_name(file, ex, i, source);
                let owned = subject_facts(ex, *stage);
                let f = owned.facts();
                let hits = self.deciding(*stage, &f);
                let merged = judge(ex, &hits, file);
                let decided = if is_builtin {
                    judge(ex, &builtin.deciding(*stage, &f), file)
                } else {
                    merged.clone()
                };
                if !decided.is_empty() {
                    r.failed += 1;
                    r.lines.push(format!("{name}: {}", decided.join("; ")));
                } else if merged.is_empty() {
                    // The merged set agrees with the built-ins: nothing to say.
                } else if let Some(h) = hits.iter().find(|h| h.source.rank < source.rank) {
                    r.overridden += 1;
                    r.lines.push(format!(
                        "{name}: overridden by {} ({})",
                        h.name(),
                        h.source.label
                    ));
                } else if let Some((entry, (by, label))) =
                    builtin.deciding(*stage, &f).iter().find_map(|h| {
                        self.disabled
                            .get_key_value(&h.name())
                            .or_else(|| self.disabled.get_key_value(&h.file))
                    })
                {
                    r.disabled += 1;
                    r.lines
                        .push(format!("{name}: disabled by {entry} ({by}, {label})"));
                } else {
                    r.failed += 1;
                    r.lines.push(format!("{name}: {}", merged.join("; ")));
                }
            }
        }
        r
    }
}

fn qualify(file: &str, rule: &str) -> String {
    if rule.contains(':') {
        rule.to_string()
    } else {
        format!("{file}:{rule}")
    }
}

/// The differences between one example's `expect` and the rules that fired.
fn judge(ex: &Example, hits: &[&Compiled], file: &str) -> Vec<String> {
    let mut fails: Vec<String> = vec![];
    let Some(e) = &ex.expect else {
        if !hits.is_empty() {
            fails.push(format!(
                "expected no match, got {}",
                hits.iter().map(|r| r.name()).collect::<Vec<_>>().join(",")
            ));
        }
        return fails;
    };
    // Flag stages compare the full set, where an empty expectation is legal.
    let flag_stage = e.classes.is_some() || e.flags.is_some();
    if hits.is_empty() && !flag_stage {
        return vec!["expected a match, got none".into()];
    }
    if let Some(first) = hits.first() {
        if let Some(rule) = &e.rule {
            let (want, got) = (qualify(file, rule), first.name());
            if want != got {
                fails.push(format!("expected rule {want}, got {got}"));
            }
        }
        if let Some(id) = &e.identity
            && first.identity.as_deref() != Some(id.as_str())
        {
            let got = first.identity.as_deref().unwrap_or("none");
            fails.push(format!("expected identity {id}, got {got}"));
        }
        if let Some(fo) = e.folder
            && first.folder != Some(fo)
        {
            let got = first.folder.map_or("none", folder_name);
            fails.push(format!("expected folder {}, got {got}", folder_name(fo)));
        }
        if let Some(ft) = &e.fold_to
            && first.fold_to.as_deref() != Some(ft)
        {
            let got = first.fold_to.as_deref().unwrap_or("none");
            fails.push(format!("expected fold_to {ft}, got {got}"));
        }
        if let Some(u) = e.owner_uid
            && first.owner_uid != Some(u)
        {
            let got = first
                .owner_uid
                .map_or_else(|| "none".to_string(), |u| u.to_string());
            fails.push(format!("expected owner_uid {u}, got {got}"));
        }
    }
    if let Some(rules) = &e.rules {
        let mut want: Vec<String> = rules.iter().map(|r| qualify(file, r)).collect();
        let mut got: Vec<String> = hits.iter().map(|r| r.name()).collect();
        want.sort();
        got.sort();
        if want != got {
            fails.push(format!(
                "expected rules [{}], got [{}]",
                want.join(", "),
                got.join(", ")
            ));
        }
    }
    if let Some(cs) = &e.classes {
        let want = cs
            .iter()
            .fold(Classes::default(), |a, c| Classes(a.0 | Classes::bit(*c)));
        let got = hits
            .iter()
            .fold(Classes::default(), |a, r| Classes(a.0 | r.classes.0));
        if want != got {
            fails.push(format!(
                "expected classes [{}], got [{}]",
                want.names().join(", "),
                got.names().join(", ")
            ));
        }
    }
    if let Some(fl) = &e.flags {
        let want = fl.iter().fold(UnitFlags::default(), |a, f| {
            UnitFlags(
                a.0 | match f {
                    UnitFlag::Lying => UnitFlags::LYING,
                    UnitFlag::Service => UnitFlags::SERVICE,
                },
            )
        });
        let got = hits
            .iter()
            .fold(UnitFlags::default(), |a, r| UnitFlags(a.0 | r.flags.0));
        if want != got {
            fails.push(format!(
                "expected flags [{}], got [{}]",
                want.names().join(", "),
                got.names().join(", ")
            ));
        }
    }
    fails
}

/// A folder as a rules file spells it, for messages a user reads.
const fn folder_name(f: FolderName) -> &'static str {
    match f {
        FolderName::Applications => "applications",
        FolderName::UserServices => "user_services",
    }
}

/// Owned facts for an example subject; `Facts` borrows from it.
pub struct OwnedFacts {
    pub comm: String,
    pub name: String,
    pub exe: Option<String>,
    pub argv: Vec<String>,
    pub cgroup: String,
    pub unit: Option<String>,
    pub script: Option<String>,
    pub identity: Option<String>,
    pub container: Option<String>,
}
impl OwnedFacts {
    #[must_use]
    pub fn facts(&self) -> Facts<'_> {
        Facts {
            comm: &self.comm,
            name: &self.name,
            exe: self.exe.as_deref(),
            argv: &self.argv,
            cgroup: &self.cgroup,
            unit: self.unit.as_deref(),
            script: self.script.as_deref(),
            identity: self.identity.as_deref(),
            container: self.container.as_deref(),
        }
    }
}

/// An example subject as the engine sees it: an explicit `unit` wins over the
/// one `identity::user_unit` reads from `cgroup`, and a class-stage subject
/// carries its script basename, which is how the anonymous-scripts rule is
/// exampled.
#[must_use]
pub fn subject_facts(ex: &Example, stage: Stage) -> OwnedFacts {
    let p = crate::types::Process {
        comm: ex.comm.clone().unwrap_or_default(),
        exe: ex.exe.clone(),
        ..crate::types::Process::default()
    };
    let name = crate::classify::name_ref(&p).to_string();
    let unit = ex.unit.clone().or_else(|| {
        if ex.cgroup.is_empty() {
            None
        } else {
            crate::identity::user_unit(&ex.cgroup)
        }
    });
    let script = if stage == Stage::Class {
        crate::classify::script_basename(&ex.cmdline)
    } else {
        None
    };
    OwnedFacts {
        comm: p.comm,
        name,
        exe: p.exe,
        argv: ex.cmdline.clone(),
        cgroup: ex.cgroup.clone(),
        unit,
        script,
        identity: ex.identity.clone(),
        container: ex.container.clone(),
    }
}

#[inline]
fn op(o: Op, p: &str, h: &str) -> bool {
    match o {
        Op::Prefix => h.get(..p.len()).is_some_and(|s| s.eq_ignore_ascii_case(p)),
        Op::Suffix => {
            h.len() >= p.len()
                && h.get(h.len() - p.len()..)
                    .is_some_and(|s| s.eq_ignore_ascii_case(p))
        }
        Op::Contains => {
            let (pb, hb) = (p.as_bytes(), h.as_bytes());
            if hb.len() < pb.len() {
                return false;
            }
            // First-byte scan in both cases, then the rest case-folded.
            let (p0, rest) = (pb[0], &pb[1..]);
            let up = p0.to_ascii_uppercase();
            let last = hb.len() - pb.len();
            hb[..=last].iter().enumerate().any(|(i, &b)| {
                (b == p0 || b == up) && hb[i + 1..i + pb.len()].eq_ignore_ascii_case(rest)
            })
        }
    }
}

#[inline]
fn hit(o: Op, pats: &[String], h: &str) -> bool {
    pats.iter().any(|p| op(o, p, h))
}

fn eval(t: &Test, f: &Facts) -> bool {
    match t {
        Test::Str(field, o, pats) => match field {
            Field::Name => hit(*o, pats, f.comm) || hit(*o, pats, f.name),
            Field::Exe => f.exe.is_some_and(|e| hit(*o, pats, e)),
            Field::Arg => f.argv.iter().any(|a| hit(*o, pats, a)),
            Field::Cgroup => hit(*o, pats, f.cgroup),
            Field::Unit => f.unit.is_some_and(|u| hit(*o, pats, u)),
            Field::Script => f.script.is_some_and(|s| hit(*o, pats, s)),
            Field::Identity => f.identity.is_some_and(|s| hit(*o, pats, s)),
            Field::Container => f.container.is_some_and(|s| hit(*o, pats, s)),
        },
        Test::Eq(field, by_len) => {
            let one = |h: &str| {
                by_len
                    .get(h.len())
                    .is_some_and(|v| v.iter().any(|p| h.eq_ignore_ascii_case(p)))
            };
            match field {
                Field::Name => one(f.comm) || one(f.name),
                Field::Unit => f.unit.is_some_and(one),
                Field::Script => f.script.is_some_and(one),
                Field::Identity => f.identity.is_some_and(one),
                Field::Container => f.container.is_some_and(one),
                // `compile_match` builds Eq only for the five fields above.
                Field::Exe | Field::Arg | Field::Cgroup => false,
            }
        }
        Test::All(v) => v.iter().all(|t| eval(t, f)),
        Test::Any(v) => v.iter().any(|t| eval(t, f)),
        Test::Not(b) => !eval(b, f),
    }
}

/// One rules directory: regular files by `fs::metadata` (follows symlinks)
/// whose name ends `.json`. A missing directory is silent; an unreadable
/// directory or file warns once and the rest still load.
#[must_use]
pub fn load_dir(
    dir: &std::path::Path,
    source: &Source,
    warnings: &mut Vec<String>,
) -> Vec<LoadedFile> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return vec![],
        Err(e) => {
            warnings.push(format!("{}: {e}", dir.display()));
            return vec![];
        }
    };
    let mut out = vec![];
    for ent in rd {
        let Ok(ent) = ent else { continue };
        let path = ent.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".json") {
            continue;
        }
        if !std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => out.push(LoadedFile {
                source: source.clone(),
                name: name.to_string(),
                text,
            }),
            Err(e) => warnings.push(format!("{}: {e}", path.display())),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    mod counting {
        use std::alloc::{GlobalAlloc, Layout, System};
        use std::cell::Cell;

        // Per thread: `cargo test` runs other tests on other threads at the
        // same time, and a global count would be counting theirs too.
        thread_local! {
            static COUNT: Cell<usize> = const { Cell::new(0) };
        }

        struct Counting;

        unsafe impl GlobalAlloc for Counting {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                let _ = COUNT.try_with(|c| c.set(c.get() + 1));
                unsafe { System.alloc(layout) }
            }
            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                unsafe { System.dealloc(ptr, layout) }
            }
        }

        #[global_allocator]
        static GLOBAL: Counting = Counting;

        pub fn allocations() -> usize {
            COUNT.with(Cell::get)
        }
    }

    fn file(name: &str, text: &str) -> LoadedFile {
        LoadedFile {
            source: Source {
                rank: 0,
                label: "xdg".into(),
            },
            name: name.into(),
            text: text.into(),
        }
    }

    #[test]
    fn builtin_files_parse_and_their_examples_pass() {
        let r = Rules::builtin();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.examples_total(), 127);
        let lines = r.check();
        assert!(lines.is_empty(), "{}", lines.join("\n"));
    }

    #[test]
    fn a_prefix_longer_than_the_haystack_or_inside_a_char_is_a_miss() {
        let r = Rules::from_files(vec![file(
            "10-x.json",
            r#"{"stage":"class","rules":[{"id":"p","match":{"name_prefix":"café"},"classes":["noise"]}]}"#,
        )]);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        let probe = |hay: &str| {
            r.classes(&Facts {
                comm: hay,
                name: hay,
                ..Facts::default()
            })
            .contains(Class::Noise)
        };
        assert!(probe("caf\u{e9}x"));
        assert!(!probe("caf"));
        assert!(!probe("c\u{e9}x"));
    }

    /// Half a file loaded is a rule set nobody wrote, so every one of these
    /// skips the whole file and leaves the others loaded.
    #[test]
    fn a_file_outside_the_grammar_is_skipped_whole() {
        for text in [
            "{ not json",
            r#"{"stage":"class","rules":[{"id":"a","match":{},"classes":["noise"]}]}"#,
            r#"{"stage":"class","rules":[{"id":"a","match":{"name":"x","exe_prefix":"/x"},"classes":["noise"]}]}"#,
            r#"{"stage":"class","rules":[{"id":"a","match":{"name":1},"classes":["noise"]}]}"#,
            r#"{"stage":"class","rules":[{"id":"a","match":{"name":[]},"classes":["noise"]}]}"#,
            r#"{"stage":"class","rules":[{"id":"a","match":{"all":[]},"classes":["noise"]}]}"#,
            r#"{"stage":"class","rules":[{"id":"a","match":{"name_prefix":""},"classes":["noise"]}]}"#,
            r#"{"stage":"class","rules":[{"id":"A","match":{"name":"x"},"classes":["noise"]}]}"#,
            r#"{"stage":"class","rules":[{"id":"a","match":{"name":"x"},"classes":["noise"]},{"id":"a","match":{"name":"y"},"classes":["noise"]}]}"#,
            r#"{"stage":"class","rules":[{"id":"a","match":{"name":"x"},"classes":["nope"]}]}"#,
            r#"{"stage":"class","rules":[{"id":"a","match":{"name":"x"},"identity":"x"}]}"#,
            r#"{"stage":"session","rules":[{"id":"a","match":{"name":"x"},"identity":"x"}]}"#,
            r#"{"stage":"class","rules":[{"id":"a","match":{"script":"x"},"classes":["generic"]}]}"#,
            r#"{"stage":"session","rules":[{"id":"a","match":{"identity":"x"},"identity":"x","folder":"applications"}]}"#,
            r#"{"stage":"placement","rules":[{"id":"a","match":{"container":"x"},"owner_uid":1,"folder":"applications"}]}"#,
            r#"{"stage":"class","disable":["x"],"rules":[]}"#,
            r#"{"stage":"class","disable":["a/b.json"],"rules":[]}"#,
            r#"{"stage":"class","rules":[],"examples":[{"comm":"x"}]}"#,
            r#"{"stage":"class","rules":[],"examples":[{"cmdline":["x"],"expect":null}]}"#,
            r#"{"stage":"class","rules":[],"examples":[{"comm":"x","pid":"1","expect":null}]}"#,
        ] {
            let r = with_builtins(vec![file("90-a.json", text)]);
            assert_eq!(r.problems.len(), 1, "{text}");
            assert_eq!(r.count(Stage::Session), 18, "the rest still load: {text}");
        }
    }

    #[test]
    fn every_string_test_folds_ascii_case_on_both_sides() {
        const TESTS: [&str; 17] = [
            "name",
            "name_prefix",
            "name_suffix",
            "name_contains",
            "exe_prefix",
            "exe_suffix",
            "exe_contains",
            "arg_prefix",
            "arg_contains",
            "cgroup_contains",
            "unit",
            "unit_prefix",
            "unit_suffix",
            "unit_contains",
            "script",
            "identity",
            "container",
        ];
        for key in TESTS {
            let (stage, output) = match key {
                "script" => (Stage::Class, r#""classes":["anonymous_script"]"#),
                "identity" => (Stage::Placement, r#""folder":"applications""#),
                "container" => (Stage::Placement, r#""owner_uid":1"#),
                _ => (Stage::Class, r#""classes":["noise"]"#),
            };
            let name = if stage == Stage::Class {
                "class"
            } else {
                "placement"
            };
            let r = Rules::from_files(vec![file(
                "10-x.json",
                &format!(
                    r#"{{"stage":"{name}","rules":[{{"id":"t","match":{{"{key}":"{}"}},{output}}}]}}"#,
                    if key == "container" { "abc" } else { "ABC" }
                ),
            )]);
            assert!(r.problems.is_empty(), "{key}: {:?}", r.problems);
            for (hay, hit) in [("abc", true), ("ABC", true), ("abd", false)] {
                let argv = [hay.to_string()];
                let f = Facts {
                    comm: hay,
                    name: hay,
                    exe: Some(hay),
                    argv: &argv,
                    cgroup: hay,
                    unit: Some(hay),
                    script: Some(hay),
                    identity: (key != "container").then_some(hay),
                    container: (key == "container").then_some(hay),
                };
                assert_eq!(r.deciding(stage, &f).len() == 1, hit, "{key} on {hay}");
            }
        }
    }

    #[test]
    fn lists_all_any_and_not_compose_and_flag_stages_union() {
        let r = Rules::from_files(vec![file(
            "10-x.json",
            r#"{"stage":"class","rules":[
                {"id":"list","match":{"name":["a","b"]},"classes":["shell"]},
                {"id":"both","match":{"all":[{"name":"b"},{"any":[{"exe_suffix":"/b"},{"arg_prefix":"--x"}]}]},"classes":["terminal"]},
                {"id":"no-exe","match":{"not":{"exe_prefix":"/usr"}},"classes":["noise"]}]}"#,
        )]);
        let classes = |comm: &str, exe: Option<&str>, argv: &[String]| {
            r.classes(&Facts {
                comm,
                name: comm,
                exe,
                argv,
                ..Facts::default()
            })
            .names()
        };
        assert_eq!(classes("a", Some("/usr/bin/a"), &[]), ["shell"]);
        assert_eq!(classes("b", Some("/usr/bin/b"), &[]), ["shell", "terminal"]);
        assert_eq!(
            classes("b", None, &["--xy".into()]),
            ["shell", "terminal", "noise"],
            "a `not` over a fact the process lacks is a hit"
        );
    }

    #[test]
    fn identity_stages_take_the_first_match_by_source_then_bytewise_name() {
        let rule = |id: &str| {
            format!(
                r#"{{"stage":"app","rules":[{{"id":"{id}","match":{{"name":"x"}},"identity":"{id}"}}]}}"#
            )
        };
        let at = |name: &str, id: &str, rank: usize| LoadedFile {
            source: if rank == usize::MAX {
                Source::builtin()
            } else {
                Source {
                    rank,
                    label: format!("rank {rank}"),
                }
            },
            name: name.into(),
            text: rule(id),
        };
        let f = Facts {
            comm: "x",
            name: "x",
            ..Facts::default()
        };
        let r = Rules::from_files(vec![
            at("01-z.json", "built", usize::MAX),
            at("10-a.json", "etc", 1),
            at("9-b.json", "nine", 0),
            at("10-c.json", "ten", 0),
        ]);
        assert_eq!(r.app(&f), Some("ten"), "XDG first, and 10-c before 9-b");
        let r = Rules::from_files(vec![
            at("01-z.json", "built", usize::MAX),
            at("99-a.json", "etc", 1),
        ]);
        assert_eq!(r.app(&f), Some("etc"), "/etc before every built-in");
    }

    #[test]
    fn a_container_rule_cannot_shadow_an_identity_pin() {
        let r = Rules::from_files(vec![file(
            "90-a.json",
            r#"{"stage":"placement","rules":[
                {"id":"own","match":{"not":{"container":"x"}},"owner_uid":1},
                {"id":"pin","match":{"identity":"htop"},"folder":"user_services"},
                {"id":"not-y","match":{"not":{"identity":"y"}},"folder":"applications"},
                {"id":"own-c","match":{"container":"c"},"owner_uid":7}]}"#,
        )]);
        assert_eq!(r.placement("htop").map(|c| c.id.as_str()), Some("pin"));
        assert_eq!(
            r.container_owner("c"),
            Some(1),
            "first owner rule, not the `not-y` pin"
        );
        assert_eq!(r.container_owner("x"), None);
    }

    #[test]
    fn a_fixture_row_is_an_example_subject_and_an_explicit_unit_wins() {
        let r = Rules::from_files(vec![file(
            "90-a.json",
            r#"{"stage":"unit","rules":[{"id":"svc","match":{"unit":"a.service"},"flags":["service"]}],
               "examples":[{"pid":1,"ppid":0,"pgrp":1,"uid":1000,"comm":"x","exe":"/usr/bin/x",
                 "cmdline":["x"],"kthread":false,"utime":1,"stime":2,
                 "cgroup":"0::/user.slice/user-1000.slice/user@1000.service/app.slice/b.service",
                 "unit":"a.service","expect":{"flags":["service"]}}]}"#,
        )]);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert!(r.check().is_empty(), "{:?}", r.check());
    }

    #[test]
    fn a_rules_directory_reads_json_files_and_what_links_to_them() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = std::env::temp_dir().join(format!("heft-rules-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("d.json")).unwrap();
        for n in ["a.json", "b.jsonc", "C.JSON", "target", "locked.json"] {
            std::fs::write(dir.join(n), "{}").unwrap();
        }
        symlink(dir.join("target"), dir.join("link.json")).unwrap();
        symlink(dir.join("gone"), dir.join("dangling.json")).unwrap();
        std::fs::set_permissions(dir.join("locked.json"), PermissionsExt::from_mode(0o000))
            .unwrap();
        // Root reads through mode 0, so the warning is only owed where it bites.
        let locked = std::fs::read(dir.join("locked.json")).is_err();
        let read = |path: &std::path::Path| {
            let mut warnings = vec![];
            let mut names: Vec<String> = load_dir(path, &Source::builtin(), &mut warnings)
                .into_iter()
                .map(|f| f.name)
                .collect();
            names.sort();
            (names, warnings.len())
        };
        let want: Vec<&str> = if locked {
            vec!["a.json", "link.json"]
        } else {
            vec!["a.json", "link.json", "locked.json"]
        };
        let linked = dir.with_extension("link");
        let _ = std::fs::remove_file(&linked);
        symlink(&dir, &linked).unwrap();
        let (names, warned) = read(&dir);
        let (through_link, _) = read(&linked);
        let (missing, missing_warned) = read(&dir.join("absent"));
        std::fs::remove_file(&linked).ok();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(names, want);
        assert_eq!(warned, usize::from(locked));
        assert_eq!(through_link, want, "a symlinked rules.d is read");
        assert!(
            missing.is_empty() && missing_warned == 0,
            "a missing directory is silent"
        );
    }

    #[test]
    fn evaluation_allocates_nothing() {
        let r = Rules::builtin();
        let argv = [
            "/usr/bin/gjs".to_string(),
            "/usr/share/gnome-shell/org.gnome.Shell.Notifications".into(),
        ];
        let f = Facts {
            comm: "gjs",
            name: "gjs-console",
            exe: Some("/usr/bin/gjs-console"),
            argv: &argv,
            cgroup: "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-x.scope",
            unit: Some("app-org.chromium.Chromium-1.scope"),
            ..Facts::default()
        };
        let before = counting::allocations();
        let session = r.session(&f).is_some();
        let app = r.app(&f).is_some();
        let classes = r.classes(&f).0;
        let lying = r.unit_flags("app-org.chromium.Chromium-1.scope").lying();
        let allocated = counting::allocations() - before;
        assert!(session && !app && classes == 0 && lying);
        assert_eq!(allocated, 0);
    }

    #[test]
    fn a_higher_source_beats_a_lower_one_whatever_the_file_names() {
        let mut user = file(
            "90-z.json",
            r#"{"stage":"session","rules":[{"id":"mine","match":{"name":"gsd-color"},"identity":"mine","folder":"applications"}]}"#,
        );
        user.source.rank = 0;
        let mut builtin = file(
            "10-a.json",
            r#"{"stage":"session","rules":[{"id":"gsd","match":{"name_prefix":"gsd-"},"identity":"gsd","folder":"user_services"}]}"#,
        );
        builtin.source = Source::builtin();
        let r = Rules::from_files(vec![builtin, user]);
        let f = Facts {
            comm: "gsd-color",
            name: "gsd-color",
            ..Facts::default()
        };
        assert_eq!(r.session(&f).map(|(i, _)| i), Some("mine"));
    }

    #[test]
    fn disable_drops_a_file_and_a_rule_across_every_source() {
        let r = with_builtins(vec![file(
            "90-a.json",
            r#"{"stage":"placement","disable":["10-classes.json:noise","05-units.json","nothing.json"],"rules":[]}"#,
        )]);
        assert!(r.is_empty(Stage::Unit));
        assert!(!r.classes_of_name("cat").contains(Class::Noise));
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
    }

    /// `disable` applies to a file name in every source, so a user file named
    /// like a built-in that disables `file:id` drops its own rule of that id.
    #[test]
    fn a_same_name_user_file_that_disables_its_own_id_loses_it() {
        let r = with_builtins(vec![file(
            "40-trinity.json",
            r#"{"stage":"session","disable":["40-trinity.json:trinity-session"],
               "rules":[{"id":"trinity-session","match":{"name":"kicker"},"identity":"mine","folder":"applications"}]}"#,
        )]);
        let kicker = Facts {
            comm: "kicker",
            name: "kicker",
            exe: Some("/opt/trinity/bin/tdeinit"),
            ..Facts::default()
        };
        assert_eq!(r.session(&kicker), None);
    }

    fn with_builtins(mut files: Vec<LoadedFile>) -> Rules {
        files.extend(builtin_files());
        Rules::from_files(files)
    }

    #[test]
    fn check_rules_fails_a_wrong_user_example_and_a_malformed_file() {
        let wrong = file(
            "90-a.json",
            r#"{"stage":"session","rules":[{"id":"x","match":{"name":"htop"},"identity":"htop","folder":"user_services"}],
               "examples":[{"comm":"htop","expect":{"rule":"x","folder":"applications"}}]}"#,
        );
        let r = with_builtins(vec![wrong, file("91-b.json", "{ nope")]).report(&Rules::builtin());
        assert_eq!(r.failed, 2, "{:#?}", r.lines);
        assert!(
            r.lines
                .iter()
                .any(|l| l.starts_with("90-a.json:x#0 (xdg): "))
        );
        assert!(r.lines.iter().any(|l| l.starts_with("91-b.json (xdg): ")));
        assert_eq!(r.examples, 128);
    }

    #[test]
    fn check_rules_reports_an_override_and_a_disable_without_failing() {
        let mine = file(
            "90-a.json",
            r#"{"stage":"session","disable":["40-trinity.json"],
               "rules":[{"id":"mine","match":{"name_prefix":"gsd-"},"identity":"mine","folder":"applications"}]}"#,
        );
        let r = with_builtins(vec![mine]).report(&Rules::builtin());
        assert_eq!(r.failed, 0, "{:#?}", r.lines);
        assert!(
            r.lines
                .iter()
                .any(|l| l.ends_with("overridden by 90-a.json:mine (xdg)")),
            "{:#?}",
            r.lines
        );
        assert!(
            r.lines
                .iter()
                .any(|l| l.ends_with("disabled by 40-trinity.json (90-a.json, xdg)")),
            "{:#?}",
            r.lines
        );
    }

    /// R44: nanoseconds per process for each stage over the gui fixture and
    /// any `HEFT_BENCH_FIXTURE`, `HEFT_BENCH_ITERS` passes each. In the crate
    /// so it builds facts through `group::facts_of`, the tick's own path. Run
    /// with `cargo test --release --lib -- --ignored --nocapture rules_timing`.
    #[test]
    #[ignore]
    #[expect(
        clippy::cast_precision_loss,
        reason = "a process count and a nanosecond total both fit in 52 bits"
    )]
    fn rules_timing() {
        use std::fmt::Write;
        #[derive(Deserialize)]
        struct Dump {
            processes: Vec<Row>,
        }
        #[derive(Deserialize)]
        struct Row {
            pid: u32,
            ppid: u32,
            comm: String,
            exe: Option<String>,
            #[serde(default)]
            cmdline: Vec<String>,
            cgroup: String,
        }
        let iters: u32 = std::env::var("HEFT_BENCH_ITERS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5000);
        let mut paths =
            vec![concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/gui/world.json").to_string()];
        paths.extend(std::env::var("HEFT_BENCH_FIXTURE"));
        let rules = Rules::builtin();
        for path in paths {
            let dump: Dump =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let procs: Vec<(crate::types::Process, Option<String>)> = dump
                .processes
                .into_iter()
                .map(|r| {
                    let unit = crate::identity::user_unit(&r.cgroup);
                    let p = crate::types::Process {
                        pid: r.pid,
                        ppid: r.ppid,
                        comm: r.comm,
                        exe: r.exe,
                        cmdline: r.cmdline,
                        cgroup: r.cgroup,
                        ..crate::types::Process::default()
                    };
                    (p, unit)
                })
                .collect();
            let n = procs.len() as f64;
            let mut acc = 0u64;
            let mut line = format!("rules_timing {path}: {} processes", procs.len());
            for (label, which) in [
                ("classes", 0),
                ("session", 1),
                ("app", 2),
                ("unit_flags", 3),
            ] {
                let t = std::time::Instant::now();
                for _ in 0..iters {
                    for (p, unit) in &procs {
                        let f = crate::group::facts_of(p, unit.as_deref());
                        acc += match which {
                            0 => u64::from(rules.classes(&f).0),
                            1 => rules.session(&f).map_or(0, |(i, _)| i.len() as u64),
                            2 => rules.app(&f).map_or(0, |i| i.len() as u64),
                            _ => f.unit.map_or(0, |u| u64::from(rules.unit_flags(u).0)),
                        };
                    }
                }
                let ns = t.elapsed().as_nanos() as f64 / f64::from(iters) / n;
                let _ = write!(line, ", {label} {ns:.0} ns");
            }
            std::hint::black_box(acc);
            println!("{line}");
        }
    }
}
