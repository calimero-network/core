"""
WASM ABI v1 Parser

Parses Calimero WASM ABI v1 schemas and extracts type and method definitions.
"""

import json
from typing import Dict, List, Any, Optional, Union
from dataclasses import dataclass
from pathlib import Path


@dataclass
class ABIType:
    """Represents an ABI type definition."""

    name: str
    kind: str
    fields: Optional[List[Dict[str, Any]]] = None
    variants: Optional[List[Dict[str, Any]]] = None
    target: Optional[Dict[str, Any]] = None
    key: Optional[Dict[str, Any]] = None
    value: Optional[Dict[str, Any]] = None
    items: Optional[Dict[str, Any]] = None
    size: Optional[int] = None
    nullable: bool = False


@dataclass
class ABIMethod:
    """Represents an ABI method definition."""

    name: str
    params: List[Dict[str, Any]]
    returns: Optional[Dict[str, Any]] = None
    returns_nullable: bool = False


@dataclass
class ABIEvent:
    """Represents an ABI event definition."""

    name: str
    payload: Optional[Dict[str, Any]] = None


@dataclass
class WASMABISchema:
    """Represents a complete WASM ABI v1 schema."""

    schema_version: str
    types: Dict[str, ABIType]
    methods: List[ABIMethod]
    events: List[ABIEvent]


class WASMABIParser:
    """Parser for Calimero WASM ABI v1 schemas."""

    def parse_file(self, file_path: Union[str, Path]) -> WASMABISchema:
        """Parse a WASM ABI schema from a file."""
        with open(file_path, "r", encoding="utf-8") as f:
            data = json.load(f)
        return self.parse(data)

    def parse(self, data: Dict[str, Any]) -> WASMABISchema:
        """Parse a WASM ABI schema from a dictionary."""
        # Parse types
        types = {}
        for type_name, type_def in data.get("types", {}).items():
            types[type_name] = self._parse_type(type_name, type_def)

        # Parse methods
        methods = []
        for method_def in data.get("methods", []):
            methods.append(self._parse_method(method_def))

        # Parse events
        events = []
        for event_def in data.get("events", []):
            events.append(self._parse_event(event_def))

        return WASMABISchema(
            schema_version=data.get("schema_version", "wasm-abi/1"),
            types=types,
            methods=methods,
            events=events,
        )

    def _parse_type(self, name: str, type_def: Dict[str, Any]) -> ABIType:
        """Parse a type definition."""
        kind = type_def.get("kind", "unknown")

        return ABIType(
            name=name,
            kind=kind,
            fields=type_def.get("fields"),
            variants=type_def.get("variants"),
            target=type_def.get("target"),
            key=type_def.get("key"),
            value=type_def.get("value"),
            items=type_def.get("items"),
            size=type_def.get("size"),
            nullable=type_def.get("nullable", False),
        )

    def _parse_method(self, method_def: Dict[str, Any]) -> ABIMethod:
        """Parse a method definition."""
        return ABIMethod(
            name=method_def.get("name", ""),
            params=method_def.get("params", []),
            returns=method_def.get("returns"),
            returns_nullable=method_def.get("returns_nullable", False),
        )

    def _parse_event(self, event_def: Dict[str, Any]) -> ABIEvent:
        """Parse an event definition."""
        return ABIEvent(
            name=event_def.get("name", ""), payload=event_def.get("payload")
        )
