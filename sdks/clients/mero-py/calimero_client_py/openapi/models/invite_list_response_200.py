from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.invite_list_response_200_invites_item import (
        InviteListResponse200InvitesItem,
    )


T = TypeVar("T", bound="InviteListResponse200")


@_attrs_define
class InviteListResponse200:
    """
    Attributes:
        invites (list['InviteListResponse200InvitesItem']):
    """

    invites: list["InviteListResponse200InvitesItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.invite_list_response_200_invites_item import (
            InviteListResponse200InvitesItem,
        )

        invites = []
        for invites_item_data in self.invites:
            invites_item = invites_item_data.to_dict()
            invites.append(invites_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "invites": invites,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.invite_list_response_200_invites_item import (
            InviteListResponse200InvitesItem,
        )

        d = dict(src_dict)
        invites = []
        _invites = d.pop("invites")
        for invites_item_data in _invites:
            invites_item = InviteListResponse200InvitesItem.from_dict(invites_item_data)

            invites.append(invites_item)

        invite_list_response_200 = cls(
            invites=invites,
        )

        invite_list_response_200.additional_properties = d
        return invite_list_response_200

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
