# dflog

dflog is a command-line tool that keeps a tamper-evident log of the steps an
examiner takes during a digital forensic investigation.

You run your usual tools through dflog. For every step it records the exact
command line, the reason you give, start and end times, the exit status, the
complete stdout and stderr, and which files in the case were created,
modified or deleted, each with its hash. Entries are hash-chained, so editing,
removing or reordering an entry afterwards is detected. When the case is
done, dflog exports the log as a text, HTML or Markdown report.

```console
$ dflog init --examiner "Jane Doe" --case-id 2026-0042 case-0042
$ cd case-0042/working
$ cp /mnt/evidence/disk.img .
$ dflog record -m "Copied disk image from evidence drive"
$ dflog run -m "Extract strings for keyword search" -- strings -n 8 disk.img
$ dflog verify
$ dflog export -f html
```

## Installation

dflog needs Rust 1.89 or newer and runs on Unix-like systems (Linux, macOS).

```console
$ cargo install --git https://github.com/Davvue/dflog
```

Or, from a clone of this repository:

```console
$ cargo install --path .
```

## Workflow

### 1. Create a workspace

```console
$ dflog init --examiner "Jane Doe" --case-id 2026-0042 [DIR]
```

This creates the workspace in `DIR` (default: the current directory):

```text
DIR/
├── .dflog/
│   ├── config.toml     case settings
│   ├── journal.jsonl   the hash-chained journal
│   ├── index.json      cache of file hashes, used to detect changes
│   └── cmd-out/        captured stdout and stderr of every command
├── working/            put your case files here
└── reports/            exported reports
```

The first journal entry records the case configuration. Options:

- `--hash sha256|sha512`: hash algorithm for the case (default `sha256`).
  Don't change it in `config.toml` later, see [Known limitations](#known-limitations).
- `-m <MESSAGE>`: message for the first entry.

dflog only watches `working/`. Every other command can be run from anywhere
inside the workspace, or from elsewhere with `-C <DIR>`.

### 2. Run commands through dflog

```console
$ dflog run -m "Why you are doing this" -- <command> [args...]
```

The message is required: a step without a reason is a gap in the record.
Everything after `--` is the command, so its options are never mistaken for
dflog's.

dflog then:

1. hashes `working/` and notes any changes made since the last entry,
2. runs the command, showing its output live while also saving stdout and
   stderr under `.dflog/cmd-out/`,
3. hashes `working/` again and records what the command created, modified or
   deleted,
4. appends the entry to the journal. It also stores the resolved path and
   hash of the program that ran, so you can show which tool version was used.

dflog exits with the command's exit code: 127 if the command couldn't be
started, or 128 + N if it was killed by signal N. Failed and interrupted runs
are recorded as well. Ctrl-C goes to the command, and dflog still writes the
entry.

If the journal is damaged or can't be written, `run` refuses to start the
command at all, so nothing happens without being recorded.

### 3. Record manual changes

Some steps can't be wrapped: editing a file in a hex editor, copying files
in a file manager, unpacking an archive with a GUI tool. Do the step, then:

```console
$ dflog record -m "Fixed partition table offset in disk.img with a hex editor"
```

dflog compares `working/` against the last entry and records every
difference. If nothing changed, no entry is written.

Changes you don't record are not lost: the next `run` or `record` finds them
and lists them as *external changes*, made outside dflog at some point since
the previous entry.

### 4. Review and verify

```console
$ dflog journal     # print every entry in readable form
$ dflog verify      # check the hash chain
```

`verify` checks every entry from the first to the last and exits with status
1 if anything is broken, naming the first line that fails. If the journal is
intact, it prints the current **head hash**, the hash of the last entry.

> [!IMPORTANT]
> Write the head hash down outside the workspace: in your case notes, a
> ticket, or an email to a colleague. The chain detects changes to existing
> entries, but if entries are removed **from the end** of the journal, the
> remaining chain is still valid. Only comparing the head hash with one
> recorded earlier shows that.

### 5. Export a report

```console
$ dflog export                      # text report in reports/
$ dflog export -f html              # or: markdown
$ dflog export -o /path/report.md   # format taken from the extension
```

Reports contain the case details, the verification result, the hash of the
journal file, and every entry with its changes. Without `-o`, reports are
named after the current UTC time (configurable as `name-format` under
`[report]` in `config.toml`). Existing files are never overwritten. HTML
reports are a single file with no scripts or external resources, so they open
offline and print cleanly.

### Rebuilding the index

`.dflog/index.json` is only a cache: it lets dflog skip rehashing unchanged
files. If it's deleted or damaged, `run` and `record` stop with an error.
Rebuild it with:

```console
$ dflog reindex -m "Index file was deleted by a cleanup script"
```

`reindex` first verifies the journal. It then works out from the journal what
`working/` should contain, compares that against a fresh hash of every file,
and writes a `reindex` entry listing every difference. So a lost index never
lets changes go unrecorded.

## How the journal works

The journal (`.dflog/journal.jsonl`) is a plain-text file with one JSON
object per line, so it can be inspected with standard tools. Every entry
contains a `prev` field with the hash of the previous line, for example
`"sha256:9f86d0…"`. Changing any byte of an entry changes its hash, which no
longer matches the `prev` of the next entry. Each hash names its algorithm,
so the chain can be checked without the config file.

Files are hashed in full. Symbolic links are recorded with their target and
never followed. Files dflog can't read (permissions, special files) are
recorded with the error instead of a hash, so they show up rather than being
skipped.

## Known limitations

- **Anything that changes `working/` during a run is attributed to that
  run.** dflog sees changes, not who made them. Don't let other programs
  write to `working/` while a command runs, and don't run two dflog commands
  in the same workspace at the same time.
- **Only `working/` is watched.** Changes elsewhere, e.g. a device a command
  writes to directly, are not recorded as file changes. The command line,
  output and exit status are still logged.
- **The command's output is captured through pipes, not a terminal.** Most
  tools behave the same, but some drop colors, buffer their output
  differently, or refuse to run interactively. Don't use `dflog run` for
  full-screen programs such as editors or pagers; run them directly and use
  `dflog record` afterwards.
- **File names must be valid UTF-8.** If `working/` contains a file whose name
  isn't, `run`, `record` and `reindex` stop with an error naming the file.
- **Don't change the hash algorithm after `init`.** dflog compares hashes
  as recorded, so after a switch every file appears as modified.
- **Unix only.** Windows is not supported.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.