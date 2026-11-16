from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

T = TypeVar("T", bound="OutboundApproval")


@_attrs_define
class OutboundApproval:
    """What an owner approved for one publisher's apps to call out to: the approved URL prefixes, and every prefix it was
    shown.

        Attributes:
            prefixes (list[str]):
            reviewed (list[str]):
    """

    prefixes: list[str]
    reviewed: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        prefixes = self.prefixes

        reviewed = self.reviewed

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "prefixes": prefixes,
                "reviewed": reviewed,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        prefixes = cast(list[str], d.pop("prefixes"))

        reviewed = cast(list[str], d.pop("reviewed"))

        outbound_approval = cls(
            prefixes=prefixes,
            reviewed=reviewed,
        )

        outbound_approval.additional_properties = d
        return outbound_approval

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
