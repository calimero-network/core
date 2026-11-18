from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.presence_get_response_200_present_item import (
        PresenceGetResponse200PresentItem,
    )


T = TypeVar("T", bound="PresenceGetResponse200")


@_attrs_define
class PresenceGetResponse200:
    """
    Attributes:
        present (list['PresenceGetResponse200PresentItem']):
    """

    present: list["PresenceGetResponse200PresentItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.presence_get_response_200_present_item import (
            PresenceGetResponse200PresentItem,
        )

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
        from ..models.presence_get_response_200_present_item import (
            PresenceGetResponse200PresentItem,
        )

        d = dict(src_dict)
        present = []
        _present = d.pop("present")
        for present_item_data in _present:
            present_item = PresenceGetResponse200PresentItem.from_dict(
                present_item_data
            )

            present.append(present_item)

        presence_get_response_200 = cls(
            present=present,
        )

        presence_get_response_200.additional_properties = d
        return presence_get_response_200

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
