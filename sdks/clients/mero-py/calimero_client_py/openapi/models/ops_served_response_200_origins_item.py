from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="OpsServedResponse200OriginsItem")


@_attrs_define
class OpsServedResponse200OriginsItem:
    """
    Attributes:
        origin (str):
        host (str):
        label (str):
        added_at (str):
    """

    origin: str
    host: str
    label: str
    added_at: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        origin = self.origin

        host = self.host

        label = self.label

        added_at = self.added_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "origin": origin,
                "host": host,
                "label": label,
                "addedAt": added_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        origin = d.pop("origin")

        host = d.pop("host")

        label = d.pop("label")

        added_at = d.pop("addedAt")

        ops_served_response_200_origins_item = cls(
            origin=origin,
            host=host,
            label=label,
            added_at=added_at,
        )

        ops_served_response_200_origins_item.additional_properties = d
        return ops_served_response_200_origins_item

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
