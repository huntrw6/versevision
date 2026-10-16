# Graph Report - .  (2026-07-18)

## Corpus Check
- Corpus is ~8,804 words - fits in a single context window. You may not need a graph.

## Summary
- 159 nodes · 353 edges · 12 communities (10 shown, 2 thin omitted)
- Extraction: 91% EXTRACTED · 8% INFERRED · 0% AMBIGUOUS · INFERRED: 30 edges (avg confidence: 0.71)
- Token cost: 1,190 input · 2,920 output

## Community Hubs (Navigation)
- Presentation Safety and Models
- Reference Parsing Core
- CLI and Document Extraction
- Architecture and Feasibility
- Parser Tests and Normalization
- Proto Binding Generation
- macOS Packaging
- Project Configuration

## God Nodes (most connected - your core abstractions)
1. `_Cursor` - 15 edges
2. `_parse_after_book()` - 14 edges
3. `parse_references()` - 14 edges
4. `ProtobufPresentationWriter` - 13 edges
5. `parse_command()` - 9 edges
6. `WriteResult` - 8 edges
7. `VerseSpan` - 8 edges
8. `PassageReference` - 8 edges
9. `Token` - 8 edges
10. `Book` - 7 edges

## Surprising Connections (you probably didn't know these)
- `Direct .pro Writing Safety Gate` --semantically_similar_to--> `ProPresenter Compatibility Gate`  [INFERRED] [semantically similar]
  README.md → docs/FEASIBILITY.md
- `ProPresenter Compatibility Gate` --semantically_similar_to--> `Unofficial Schema Safety Policy`  [INFERRED] [semantically similar]
  docs/FEASIBILITY.md → proto/README.md
- `test_unknown_application_version_is_rejected_without_changes()` --calls--> `ProtobufPresentationWriter`  [INFERRED]
  tests/integration/test_protobuf_writer.py → src/versevision/presentation/protobuf_writer.py
- `test_warnings_are_not_shared()` --calls--> `PassageReference`  [INFERRED]
  tests/unit/test_models.py → src/versevision/references/models.py
- `test_range_delimiters_and_version_case_are_stable()` --calls--> `parse_references()`  [INFERRED]
  tests/unit/test_parser_properties.py → src/versevision/references/parser.py

## Import Cycles
- None detected.

## Hyperedges (group relationships)
- **Direct .pro Writing Compatibility Evidence** — readme_direct_pro_writing_gate, docs_feasibility_windows_feasibility_spike, docs_feasibility_compatibility_gate, proto_readme_unofficial_schema_safety_policy [INFERRED 0.95]
- **ProPresenter Schema Binding Chain** — proto_source_propresenter19_readme_v19_beta_proto_dump, proto_readme_proto_19beta_schema_snapshot, proto_readme_minimal_python_binding_generation [INFERRED 0.95]

## Communities (12 total, 2 thin omitted)

### Community 0 - "Presentation Safety and Models"
Cohesion: 0.12
Nodes (21): Message, Protocol, create_backup(), Path, restore_backup(), PresentationInfo, PresentationWriter, BaseModel (+13 more)

### Community 1 - "Reference Parsing Core"
Cohesion: 0.18
Nodes (19): Book, recognize_book(), format_reference(), _point(), ParseIssue, ParseResult, PassageReference, BaseModel (+11 more)

### Community 3 - "CLI and Document Extraction"
Cohesion: 0.14
Nodes (15): Argument, Exception, help, Option, main(), parse_command(), Path, VerseVision command line application. (+7 more)

### Community 4 - "Architecture and Feasibility"
Cohesion: 0.14
Nodes (16): Atomic Presentation Write Workflow, ProPresenter Compatibility Gate, Milestone 0 Feasibility Report, Remaining Production Support Requirements, Windows ProPresenter Feasibility Spike, Minimal Python Binding Generation, ProPresenter Protobuf Bindings, Proto 19beta Schema Snapshot (+8 more)

### Community 5 - "Parser Tests and Normalization"
Cohesion: 0.22
Nodes (9): normalize_fragment(), parse_references(), test_range_delimiters_and_version_case_are_stable(), test_composite_is_one_logical_reference(), test_invalid_references_are_reported(), test_semicolon_inherits_book_as_new_passage(), test_supported_references(), test_unrecognized_books_are_ignored() (+1 more)

### Community 6 - "Proto Binding Generation"
Cohesion: 0.67
Nodes (3): dependency_closure(), main(), Generate the minimal Python binding closure needed for Presentation files.

## Ambiguous Edges - Review These
- `VerseVision CLI` → `Windows ProPresenter Feasibility Spike`  [AMBIGUOUS]
  README.md · relation: conceptually_related_to

## Knowledge Gaps
- **9 isolated node(s):** `versevision`, `build_macos.sh script`, `Grammar-Based Bible Reference Parser`, `Supported Document Inputs`, `Milestone 0 Feasibility Report` (+4 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **2 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **What is the exact relationship between `VerseVision CLI` and `Windows ProPresenter Feasibility Spike`?**
  _Edge tagged AMBIGUOUS (relation: conceptually_related_to) - confidence is low._
- **Why does `parse_references()` connect `Parser Tests and Normalization` to `Reference Parsing Core`, `CLI and Document Extraction`?**
  _High betweenness centrality (0.097) - this node is a cross-community bridge._
- **Why does `parse_command()` connect `CLI and Document Extraction` to `Parser Tests and Normalization`?**
  _High betweenness centrality (0.074) - this node is a cross-community bridge._
- **Why does `_parse_after_book()` connect `Reference Parsing Core` to `Parser Tests and Normalization`?**
  _High betweenness centrality (0.028) - this node is a cross-community bridge._
- **Are the 7 inferred relationships involving `_Cursor` (e.g. with `Book` and `ParseIssue`) actually correct?**
  _`_Cursor` has 7 INFERRED edges - model-reasoned connections that need verification._
- **Are the 8 inferred relationships involving `parse_references()` (e.g. with `parse_command()` and `test_range_delimiters_and_version_case_are_stable()`) actually correct?**
  _`parse_references()` has 8 INFERRED edges - model-reasoned connections that need verification._
- **Are the 6 inferred relationships involving `ProtobufPresentationWriter` (e.g. with `PresentationInfo` and `VerificationResult`) actually correct?**
  _`ProtobufPresentationWriter` has 6 INFERRED edges - model-reasoned connections that need verification._