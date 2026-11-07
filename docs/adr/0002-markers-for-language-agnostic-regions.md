# Marker comments locate Regions in any language

Arbor knows nothing about specific languages, so the Scaffolder brackets each Region with Marker comments carrying the Work Unit ID (e.g. `// ARBOR:BEGIN WU-7` … `// ARBOR:END WU-7`), and Arbor splices Writer output between them by plain text matching. As a sanity check, the Writer also returns exactly 2 unchanged lines of context on each side, and they must match the file. Markers are stripped when a Work Unit passes, and a Session can't finish while any remain. Intermediate commits may still contain Markers, so squashing is offered by default at merge/PR time.

## Considered Options

- **Out-of-file anchors (line ranges or surrounding text recorded in the Work Unit)**: rejected. Region boundaries can't be found reliably without parsing the language, and earlier Work Units shift the surrounding text.
- **tree-sitter**: deferred. It may replace Markers later for the languages it supports.
