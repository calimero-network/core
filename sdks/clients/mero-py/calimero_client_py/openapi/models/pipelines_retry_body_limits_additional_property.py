from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="PipelinesRetryBodyLimitsAdditionalProperty")


@_attrs_define
class PipelinesRetryBodyLimitsAdditionalProperty:
    """
    Attributes:
        fuel (Union[Unset, int]):
        memory_mib (Union[Unset, int]):
        wall_secs (Union[Unset, int]):
    """

    fuel: Union[Unset, int] = UNSET
    memory_mib: Union[Unset, int] = UNSET
    wall_secs: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        fuel = self.fuel

        memory_mib = self.memory_mib

        wall_secs = self.wall_secs

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if fuel is not UNSET:
            field_dict["fuel"] = fuel
        if memory_mib is not UNSET:
            field_dict["memoryMib"] = memory_mib
        if wall_secs is not UNSET:
            field_dict["wallSecs"] = wall_secs

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        fuel = d.pop("fuel", UNSET)

        memory_mib = d.pop("memoryMib", UNSET)

        wall_secs = d.pop("wallSecs", UNSET)

        pipelines_retry_body_limits_additional_property = cls(
            fuel=fuel,
            memory_mib=memory_mib,
            wall_secs=wall_secs,
        )

        pipelines_retry_body_limits_additional_property.additional_properties = d
        return pipelines_retry_body_limits_additional_property

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
