from __future__ import annotations
import hashlib
from pathlib import Path
from typing import Iterable
import pandas as pd
import pyarrow as pa
import pyarrow.parquet as pq
from axon.core.graph.model import GraphNode

class ArrowExporter:
    """Exporter for Axon graph nodes to Apache Arrow/Parquet format."""

    def export_nodes(self, nodes: Iterable[GraphNode], output_path: Path) -> None:
        """Export nodes to a Parquet file with the schema required by HydraDB."""
        data = []
        for node in nodes:
            # metadata_hash: hash of content and properties
            meta_str = f"{node.content}{str(node.properties)}"
            meta_hash = hashlib.sha256(meta_str.encode()).hexdigest()
            
            data.append({
                "id": node.id,
                "name": node.name,
                "path": node.file_path,
                "line_range": f"{node.start_line}-{node.end_line}",
                "kind": node.label.value if hasattr(node.label, 'value') else str(node.label),
                "metadata_hash": meta_hash
            })
        
        df = pd.DataFrame(data)
        table = pa.Table.from_pandas(df)
        pq.write_table(table, str(output_path))
