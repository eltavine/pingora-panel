"""Schema-versioned policy document loading.

Readers register against the ``schema_version`` they understand, mirroring the
snapshot codec registry in `panel/snapshot-store-fs`. A future format is added
by registering an additional reader, so introducing a new schema never requires
editing the guards that already consume an older one.

Readers receive an evaluation context (typically the policy date) so that
loading and validating a document is a single pass over the file.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Callable, Generic, TypeVar

from .errors import PolicyError

Context = TypeVar("Context")
Document = TypeVar("Document")

DocumentReader = Callable[[dict, Context], Document]


class DocumentRegistry(Generic[Context, Document]):
    """Selects a reader for one policy document by its declared schema version."""

    def __init__(self, description: str) -> None:
        self._description = description
        self._readers: dict[int, DocumentReader[Context, Document]] = {}

    def reader(
        self, schema_version: int
    ) -> Callable[
        [DocumentReader[Context, Document]], DocumentReader[Context, Document]
    ]:
        if schema_version < 1:
            raise ValueError("policy schema versions start at 1")

        def register(
            read: DocumentReader[Context, Document],
        ) -> DocumentReader[Context, Document]:
            if schema_version in self._readers:
                raise ValueError(
                    f"{self._description} schema {schema_version} is already registered"
                )
            self._readers[schema_version] = read
            return read

        return register

    def supported_versions(self) -> tuple[int, ...]:
        return tuple(sorted(self._readers))

    def load(self, path: Path, context: Context) -> Document:
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as error:
            raise PolicyError(f"cannot read {self._description} {path}: {error}") from error
        if not isinstance(document, dict):
            raise PolicyError(f"{self._description} must be a JSON object: {path}")
        version = document.get("schema_version")
        if not isinstance(version, int) or isinstance(version, bool):
            raise PolicyError(
                f"{self._description} must declare an integer schema_version: {path}"
            )
        read = self._readers.get(version)
        if read is None:
            raise PolicyError(
                f"{self._description} schema {version} is not supported "
                f"(supported: {list(self.supported_versions())})"
            )
        return read(document, context)


__all__ = ["Context", "Document", "DocumentReader", "DocumentRegistry"]
