from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="OpsPoolsSetBodyPoolsAdditionalProperty")


@_attrs_define
class OpsPoolsSetBodyPoolsAdditionalProperty:
    """
    Attributes:
        runs_per_node (int):
        max_memory_mib (int):
    """

    runs_per_node: int
    max_memory_mib: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        runs_per_node = self.runs_per_node

        max_memory_mib = self.max_memory_mib

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "runsPerNode": runs_per_node,
                "maxMemoryMib": max_memory_mib,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        runs_per_node = d.pop("runsPerNode")

        max_memory_mib = d.pop("maxMemoryMib")

        ops_pools_set_body_pools_additional_property = cls(
            runs_per_node=runs_per_node,
            max_memory_mib=max_memory_mib,
        )

        ops_pools_set_body_pools_additional_property.additional_properties = d
        return ops_pools_set_body_pools_additional_property

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
