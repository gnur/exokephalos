# Repository Import and Export

exokephalos provides subcommands to import raw markdown files into the repository and export them back for use elsewhere.

## Import Command

The `import` command reads raw markdown files from a source directory, parses their frontmatter, normalizes them, and writes them to the structured exokephalos directory layout.

### Usage

```bash
EXO_DIR=/path/to/your/notes xo import <source-directory> <type>
```

For example, to import a directory of raw notes as the `note` type:

```bash
EXO_DIR=/path/to/your/notes xo import ~/Desktop/my-old-notes note
```

### What Happens During Import?

1. **ID Generation**: If the raw markdown file lacks an `id` field in its frontmatter, a new unique, case-insensitive lowercase base32 ID is generated.
2. **Metadata Injection**: The command ensures that essential metadata fields (`id`, `type`, `created`, `tags`, and `title`) exist in the YAML frontmatter. If `created` is missing, the file's creation/modification time is parsed and format-preserved.
3. **Structured Storage**: Imported files are saved under the exokephalos workspace path (`EXO_DIR`) using the following folder structure:
   `<EXO_DIR>/<type>/<year>/<month>/<slugified-title>.md`
4. **YAML Timestamps**: YAML date/time values are converted to unquoted `!!timestamp` tags.

---

## Export Command

The `export` command copies files from your exokephalos repository into a target directory while clean-formatting them to remove application-specific metadata.

### Usage

```bash
EXO_DIR=/path/to/your/notes xo export <output-directory> [--type <type>]
```

For example, to export all items in the repository:

```bash
EXO_DIR=/path/to/your/notes xo export ~/Desktop/my-exported-workspace
```

To export only `note` type items:

```bash
EXO_DIR=/path/to/your/notes xo export ~/Desktop/my-exported-notes --type note
```

### What Happens During Export?

1. **Frontmatter Cleanup**: All application-specific frontmatter metadata fields—specifically `id`, `type`, and `created`—are removed from the frontmatter block, resulting in a cleaner, standard markdown file.
2. **Directory Structure**: Files are written to the target directory matching the type and date path:
   `<output-directory>/<type>/<year>/<month>/<slugified-title>.md`
3. **Conflict Resolution**: If two items would export to the exact same file path (due to identical titles in the same month), the exporter resolves the conflict by appending a suffix increment (e.g., `-1.md`, `-2.md`) to the file name.
