# KuzuDB Schema: SOLL & IST Layers

## Node Tables

### SOLL Layer (Human Intent)
- **Vision**: `title (PK)`, `description`, `goal`
- **Phase**: `name (PK)`, `goal`, `status`
- **Requirement**: `id (PK)`, `title`, `description`, `justification`, `priority`
- **Concept**: `name (PK)`, `explanation`, `rationale`
- **Decision**: `id (PK)`, `title`, `impact`, `rationale`

### IST Layer (Physical Reality)
- **Project**: `name (PK)`
- **File**: `path (PK)`, `project_slug`, `size`, `priority`, `mtime`, `status`, `worker_id`
- **Symbol**: `id (PK)`, `name`, `project_slug`, `kind`, `tested`, `is_public`, `is_unsafe`, `is_nif`, `is_entry_point`, `embedding`

## Relationship Tables

### Structural (IST)
- **CONTAINS**: File -> Symbol
- **CALLS**: Symbol -> Symbol
- **CALLS_NIF**: Symbol -> Symbol
- **DEPENDS_ON**: Project -> Project
- **BELONGS_TO**: File -> Project

### Traceability (SOLL <-> IST)
- **CONTRIBUTES_TO**: Phase -> Vision
- **REFINES**: Requirement -> Phase
- **EXPLAINS**: Concept -> Requirement
- **SUBSTANTIATES**: Concept -> Symbol (The bridge)
- **ADDRESSES**: Decision -> Requirement
- **SUPERSEDES**: Concept -> Concept (Versioning)
- **AFFECTS**: Decision -> Symbol
