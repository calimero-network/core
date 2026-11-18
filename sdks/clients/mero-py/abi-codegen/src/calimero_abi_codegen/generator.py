"""
Client Code Generator

Generates Python client code from WASM ABI schemas.
"""

from typing import Dict, List, Any, Optional
from pathlib import Path
from .parser import WASMABISchema, ABIMethod, ABIType
from .type_mapper import MODULE_NAMES, TypeMapper, checked_identifier


class ClientGenerator:
    """Generates Python client code from WASM ABI schemas."""

    def __init__(self, schema: WASMABISchema):
        self.schema = schema
        self.type_mapper = TypeMapper(schema)

    def generate_types_file(self) -> str:
        """Generate the types.py file with all ABI type definitions."""
        lines = [
            '"""Generated types from WASM ABI schema"""',
            "",
            "from typing import Dict, List, Any, Optional, Union",
            "from dataclasses import dataclass",
            "from enum import Enum",
            "",
        ]

        # Generate dataclasses for record types
        for type_name, abi_type in self.schema.types.items():
            if abi_type.kind == "record":
                dataclass_code = self.type_mapper.generate_dataclass(abi_type)
                if dataclass_code:
                    lines.append(dataclass_code)
                    lines.append("")

        # Generate enums for variant types
        for type_name, abi_type in self.schema.types.items():
            if abi_type.kind == "variant":
                enum_code = self.type_mapper.generate_enum(abi_type)
                if enum_code:
                    lines.append(enum_code)
                    lines.append("")

        return "\n".join(lines)

    def generate_client_file(self, class_name: str = "ABIClient") -> str:
        """Generate the main client file."""
        class_name = checked_identifier(class_name, "class name", MODULE_NAMES)
        lines = [
            '"""Generated client from WASM ABI schema"""',
            "",
            "from typing import Dict, List, Any, Optional, Union",
            "import json",
            "from calimero import CloudClient, CloudError",
            "",
            "from .types import *",
            "",
            f"class {class_name}:",
            '    """Generated client for WASM ABI methods"""',
            "",
            "    def __init__(self, client: CloudClient, app_id: str):",
            '        """Initialize with a signed-in CloudClient and the app instance to call"""',
            "        self.client = client",
            "        self.app_id = app_id",
            "",
        ]

        # Generate methods
        for method in self.schema.methods:
            method_code = self._generate_method(method)
            lines.append(method_code)
            lines.append("")

        return "\n".join(lines)

    def _generate_method(self, method: ABIMethod) -> str:
        """Generate a single method implementation."""
        lines = []

        # Method signature
        signature = self.type_mapper.get_method_signature(method)
        lines.append(f"    {signature}")

        # Docstring
        docstring = self.type_mapper.get_method_docstring(method)
        lines.append(docstring)

        # Method body: a committed call through the app's Raft log. The ABI does
        # not mark read-only methods, so `CloudClient.view` is left to the caller.
        # A CloudError propagates as is: its args are (message, status, code, details).
        param_names = [param.get("name") for param in method.params]
        params_dict = (
            "{" + ", ".join([f"{name!r}: {name}" for name in param_names]) + "}"
        )
        lines.append(f"        args = {params_dict}")
        lines.append(f"        result = self.client.call(")
        lines.append(f"            self.app_id, {method.name!r}, args")
        lines.append(f'        )["result"]')

        # Handle return value
        if method.returns:
            lines.append("        return result")
        else:
            lines.append("        return None")

        return "\n".join(lines)

    def generate_init_file(self, class_name: str = "ABIClient") -> str:
        """Generate __init__.py file."""
        class_name = checked_identifier(class_name, "class name", MODULE_NAMES)
        lines = [
            '"""Generated ABI client package"""',
            "",
            f"from .client import {class_name}",
            "from .types import *",
            "",
            "__all__ = [",
            f"    {class_name!r},",
        ]

        # Add all type names
        for type_name in self.schema.types.keys():
            lines.append(f"    {type_name!r},")

        lines.extend(
            [
                "]",
                "",
                f"__version__ = {self.schema.schema_version!r}",
            ]
        )

        return "\n".join(lines)

    def generate_all(
        self, output_dir: Path, class_name: str = "ABIClient"
    ) -> Dict[str, str]:
        """Generate all files for the client package."""
        results = {}

        # Generate types.py
        types_code = self.generate_types_file()
        results["types.py"] = types_code

        # Generate client.py
        client_code = self.generate_client_file(class_name)
        results["client.py"] = client_code

        # Generate __init__.py
        init_code = self.generate_init_file(class_name)
        results["__init__.py"] = init_code

        return results
