from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

T = TypeVar("T", bound="UpdateViewAvailableType0EgressType0")


@_attrs_define
class UpdateViewAvailableType0EgressType0:
    """
    Attributes:
        before (list[str]):
        after (list[str]):
        added (list[str]):
        removed (list[str]):
    """

    before: list[str]
    after: list[str]
    added: list[str]
    removed: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        before = self.before

        after = self.after

        added = self.added

        removed = self.removed

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "before": before,
                "after": after,
                "added": added,
                "removed": removed,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        before = cast(list[str], d.pop("before"))

        after = cast(list[str], d.pop("after"))

        added = cast(list[str], d.pop("added"))

        removed = cast(list[str], d.pop("removed"))

        update_view_available_type_0_egress_type_0 = cls(
            before=before,
            after=after,
            added=added,
            removed=removed,
        )

        update_view_available_type_0_egress_type_0.additional_properties = d
        return update_view_available_type_0_egress_type_0

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
