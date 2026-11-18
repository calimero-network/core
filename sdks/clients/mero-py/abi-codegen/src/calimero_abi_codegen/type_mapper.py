"""
Type Mapper for WASM ABI to Python

Maps WASM ABI types to Python type annotations and generates dataclass definitions.
"""

import keyword
from typing import Dict, List, Any, Optional, Union
from dataclasses import dataclass
from .parser import WASMABISchema, ABIType, ABIMethod

# Names the generated modules import, or read in annotations, at module level.
MODULE_NAMES = frozenset(
    {"Any", "Dict", "List", "Optional", "Union", "dataclass", "Enum", "json"}
    | {"CloudClient", "CloudError", "bool", "int", "float", "str", "bytes"}
)
# Attributes of the generated client class.
CLIENT_MEMBERS = frozenset({"client", "app_id"})
# Names a generated method body binds or reads besides its parameters.
METHOD_LOCALS = MODULE_NAMES | {"self", "args", "result", "e"}


def checked_identifier(name: Any, what: str, reserved: frozenset = frozenset()) -> str:
    """Return `name` if it can be written into generated code as a bare name."""
    # ASCII only: identifiers are NFKC-normalised, the repr() literals are not.
    if (
        not isinstance(name, str)
        or not name.isascii()
        or not name.isidentifier()
        or keyword.iskeyword(name)
        or name.startswith("_")
        or name in reserved
    ):
        raise ValueError(f"{what} {name!r} cannot be used as a name in generated code")
    return name


class TypeMapper:
    """Maps WASM ABI types to Python types."""

    # Mapping from WASM ABI primitive types to Python types
    PRIMITIVE_TYPE_MAP = {
        "bool": "bool",
        "i32": "int",
        "i64": "int",
        "u32": "int",
        "u64": "int",
        "f32": "float",
        "f64": "float",
        "string": "str",
        "bytes": "bytes",
        "unit": "None",
    }

    def __init__(self, schema: WASMABISchema):
        self.schema = schema
        self.type_cache = {}
        self._validate_names()

    def _validate_names(self) -> None:
        """Refuse any schema name that cannot be written into generated code."""
        for key, abi_type in self.schema.types.items():
            for type_name in (key, abi_type.name):
                checked_identifier(type_name, "type name", MODULE_NAMES)
            for field in abi_type.fields or []:
                checked_identifier(field.get("name"), "field name")
            for variant in abi_type.variants or []:
                checked_identifier(variant.get("name"), "variant name")
        for method in self.schema.methods:
            checked_identifier(method.name, "method name", CLIENT_MEMBERS)
            seen = set()
            for param in method.params:
                name = checked_identifier(
                    param.get("name"), "parameter name", METHOD_LOCALS
                )
                if name in seen:
                    raise ValueError(f"duplicate parameter {name!r} in {method.name!r}")
                seen.add(name)

    def get_python_type(
        self, type_ref: Union[str, Dict[str, Any]], nullable: bool = False
    ) -> str:
        """Convert a type reference to a Python type annotation."""
        if isinstance(type_ref, str):
            # Handle $ref references
            if type_ref.startswith("$ref:"):
                ref_name = type_ref[5:]  # Remove "$ref:" prefix
            else:
                ref_name = type_ref

            if ref_name in self.schema.types:
                return self._get_complex_type(ref_name, nullable)
            else:
                return "Any"  # Unknown type reference

        if isinstance(type_ref, dict):
            return self._get_inline_type(type_ref, nullable)

        return "Any"

    def _get_inline_type(self, type_def: Dict[str, Any], nullable: bool = False) -> str:
        """Get Python type for an inline type definition."""
        kind = type_def.get("kind", "unknown")

        if kind in self.PRIMITIVE_TYPE_MAP:
            python_type = self.PRIMITIVE_TYPE_MAP[kind]
        elif kind == "list":
            items_type = self.get_python_type(type_def.get("items", {}))
            python_type = f"List[{items_type}]"
        elif kind == "map":
            key_type = self.get_python_type(type_def.get("key", {}))
            value_type = self.get_python_type(type_def.get("value", {}))
            python_type = f"Dict[{key_type}, {value_type}]"
        elif kind == "alias":
            target = type_def.get("target", {})
            python_type = self.get_python_type(target)
        else:
            python_type = "Any"

        if nullable:
            return f"Optional[{python_type}]"

        return python_type

    def _get_complex_type(self, type_name: str, nullable: bool = False) -> str:
        """Get Python type for a complex type definition."""
        if type_name in self.type_cache:
            python_type = self.type_cache[type_name]
        else:
            abi_type = self.schema.types[type_name]
            python_type = self._convert_complex_type(abi_type)
            self.type_cache[type_name] = python_type

        if nullable:
            return f"Optional[{python_type}]"

        return python_type

    def _convert_complex_type(self, abi_type: ABIType) -> str:
        """Convert a complex ABI type to Python type."""
        if abi_type.kind in ("record", "variant"):
            return abi_type.name
        elif abi_type.kind == "alias":
            if abi_type.target:
                return self.get_python_type(abi_type.target)
            return "Any"
        else:
            return "Any"

    def generate_dataclass(self, abi_type: ABIType) -> str:
        """Generate a Python dataclass for a record type."""
        if abi_type.kind != "record":
            return ""

        lines = [
            f"@dataclass",
            f"class {abi_type.name}:",
        ]

        if not abi_type.fields:
            lines.append("    pass")
        else:
            for field in abi_type.fields:
                field_name = field.get("name")
                field_type_ref = field.get("type", {})
                field_nullable = field.get("nullable", False)
                field_python_type = self.get_python_type(field_type_ref, field_nullable)

                lines.append(f"    {field_name}: {field_python_type}")

        return "\n".join(lines)

    def generate_enum(self, abi_type: ABIType) -> str:
        """Generate a Python enum for a variant type."""
        if abi_type.kind != "variant":
            return ""

        lines = [
            f"class {abi_type.name}(Enum):",
        ]

        if abi_type.variants:
            for variant in abi_type.variants:
                variant_name = variant.get("name")
                lines.append(f"    {variant_name} = {variant_name!r}")

        return "\n".join(lines)

    def get_method_signature(self, method: ABIMethod) -> str:
        """Generate Python method signature for an ABI method."""
        params = ["self"]

        # Add method parameters
        for param in method.params:
            param_name = param.get("name")
            param_type_ref = param.get("type", {})
            param_nullable = param.get("nullable", False)
            param_python_type = self.get_python_type(param_type_ref, param_nullable)
            params.append(f"{param_name}: {param_python_type}")

        # Add return type
        return_type = "None"
        if method.returns:
            return_python_type = self.get_python_type(
                method.returns, method.returns_nullable
            )
            return_type = return_python_type

        return f"def {method.name}({', '.join(params)}) -> {return_type}:"

    def get_method_docstring(self, method: ABIMethod) -> str:
        """Generate docstring for a method."""
        lines = [
            f'        """',
            f"        {method.name} method from ABI",
        ]

        if method.params:
            lines.append("        ")
            lines.append("        Args:")
            for param in method.params:
                param_name = param.get("name")
                param_type_ref = param.get("type", {})
                param_python_type = self.get_python_type(
                    param_type_ref, param.get("nullable", False)
                )
                lines.append(
                    f"            {param_name} ({param_python_type}): Parameter"
                )

        if method.returns:
            return_python_type = self.get_python_type(
                method.returns, method.returns_nullable
            )
            lines.append("        ")
            lines.append("        Returns:")
            lines.append(f"            {return_python_type}: Return value")

        lines.append('        """')
        return "\n".join(lines)
