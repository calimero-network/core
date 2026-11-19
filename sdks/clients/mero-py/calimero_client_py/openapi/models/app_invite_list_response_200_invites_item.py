from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="AppInviteListResponse200InvitesItem")


@_attrs_define
class AppInviteListResponse200InvitesItem:
    """
    Attributes:
        invite_key (str):
        uses_left (int):
        expires_at (int):
        expired (bool):
        created_by (str):
        created_at (int):
        scope (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
    """

    invite_key: str
    uses_left: int
    expires_at: int
    expired: bool
    created_by: str
    created_at: int
    scope: Any
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        invite_key = self.invite_key

        uses_left = self.uses_left

        expires_at = self.expires_at

        expired = self.expired

        created_by = self.created_by

        created_at = self.created_at

        scope = self.scope

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "inviteKey": invite_key,
                "usesLeft": uses_left,
                "expiresAt": expires_at,
                "expired": expired,
                "createdBy": created_by,
                "createdAt": created_at,
                "scope": scope,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        invite_key = d.pop("inviteKey")

        uses_left = d.pop("usesLeft")

        expires_at = d.pop("expiresAt")

        expired = d.pop("expired")

        created_by = d.pop("createdBy")

        created_at = d.pop("createdAt")

        scope = d.pop("scope")

        app_invite_list_response_200_invites_item = cls(
            invite_key=invite_key,
            uses_left=uses_left,
            expires_at=expires_at,
            expired=expired,
            created_by=created_by,
            created_at=created_at,
            scope=scope,
        )

        app_invite_list_response_200_invites_item.additional_properties = d
        return app_invite_list_response_200_invites_item

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
