from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="PipelineRunViewLimitsAdditionalProperty")


@_attrs_define
class PipelineRunViewLimitsAdditionalProperty:
    """
    Attributes:
        fuel (Union[None, int]):
        memory_mib (Union[None, int]):
        wall_secs (Union[None, int]):
    """

    fuel: Union[None, int]
    memory_mib: Union[None, int]
    wall_secs: Union[None, int]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        fuel: Union[None, int]
        fuel = self.fuel

        memory_mib: Union[None, int]
        memory_mib = self.memory_mib

        wall_secs: Union[None, int]
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

        def _parse_fuel(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        fuel = _parse_fuel(d.pop("fuel"))

        def _parse_memory_mib(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        memory_mib = _parse_memory_mib(d.pop("memoryMib"))

        def _parse_wall_secs(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        wall_secs = _parse_wall_secs(d.pop("wallSecs"))

        pipeline_run_view_limits_additional_property = cls(
            fuel=fuel,
            memory_mib=memory_mib,
            wall_secs=wall_secs,
        )

        pipeline_run_view_limits_additional_property.additional_properties = d
        return pipeline_run_view_limits_additional_property

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
