from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="RunCaps")


@_attrs_define
class RunCaps:
    """
    Attributes:
        fuel (int):
        memory_mib (int):
        wall_secs (int):
    """

    fuel: int
    memory_mib: int
    wall_secs: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        fuel = self.fuel

        memory_mib = self.memory_mib

        wall_secs = self.wall_secs

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "fuel": fuel,
                "memoryMib": memory_mib,
                "wallSecs": wall_secs,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        fuel = d.pop("fuel")

        memory_mib = d.pop("memoryMib")

        wall_secs = d.pop("wallSecs")

        run_caps = cls(
            fuel=fuel,
            memory_mib=memory_mib,
            wall_secs=wall_secs,
        )

        run_caps.additional_properties = d
        return run_caps

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
