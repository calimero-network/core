from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

T = TypeVar("T", bound="OpsSealedEndpointsGetResponse200")


@_attrs_define
class OpsSealedEndpointsGetResponse200:
    """
    Attributes:
        endpoints (list[str]):
    """

    endpoints: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        endpoints = self.endpoints

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "endpoints": endpoints,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        endpoints = cast(list[str], d.pop("endpoints"))

        ops_sealed_endpoints_get_response_200 = cls(
            endpoints=endpoints,
        )

        ops_sealed_endpoints_get_response_200.additional_properties = d
        return ops_sealed_endpoints_get_response_200

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
