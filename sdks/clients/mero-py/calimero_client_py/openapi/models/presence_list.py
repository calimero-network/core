from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.presence_diff import PresenceDiff


T = TypeVar("T", bound="PresenceList")


@_attrs_define
class PresenceList:
    """`GET /apps/{app}/presence`: who is here, on which device, and their state (`null`: none set).

    Attributes:
        present (list['PresenceDiff']):
    """

    present: list["PresenceDiff"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.presence_diff import PresenceDiff

        present = []
        for present_item_data in self.present:
            present_item = present_item_data.to_dict()
            present.append(present_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "present": present,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.presence_diff import PresenceDiff

        d = dict(src_dict)
        present = []
        _present = d.pop("present")
        for present_item_data in _present:
            present_item = PresenceDiff.from_dict(present_item_data)

            present.append(present_item)

        presence_list = cls(
            present=present,
        )

        presence_list.additional_properties = d
        return presence_list

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
