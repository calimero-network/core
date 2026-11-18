from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="RunFinalType0Usage")


@_attrs_define
class RunFinalType0Usage:
    """
    Attributes:
        cpu_micros (int):
        gib_micros (int):
        fuel (int):
    """

    cpu_micros: int
    gib_micros: int
    fuel: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        cpu_micros = self.cpu_micros

        gib_micros = self.gib_micros

        fuel = self.fuel

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cpuMicros": cpu_micros,
                "gibMicros": gib_micros,
                "fuel": fuel,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        cpu_micros = d.pop("cpuMicros")

        gib_micros = d.pop("gibMicros")

        fuel = d.pop("fuel")

        run_final_type_0_usage = cls(
            cpu_micros=cpu_micros,
            gib_micros=gib_micros,
            fuel=fuel,
        )

        run_final_type_0_usage.additional_properties = d
        return run_final_type_0_usage

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
