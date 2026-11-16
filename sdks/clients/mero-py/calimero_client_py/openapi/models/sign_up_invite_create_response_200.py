from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="SignUpInviteCreateResponse200")


@_attrs_define
class SignUpInviteCreateResponse200:
    """
    Attributes:
        token (str):
        invite_key (str):
        uses (int):
        expires_at (str):
        expires_at_secs (int):
        created_by (str):
        created_at (str):
    """

    token: str
    invite_key: str
    uses: int
    expires_at: str
    expires_at_secs: int
    created_by: str
    created_at: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        token = self.token

        invite_key = self.invite_key

        uses = self.uses

        expires_at = self.expires_at

        expires_at_secs = self.expires_at_secs

        created_by = self.created_by

        created_at = self.created_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "token": token,
                "inviteKey": invite_key,
                "uses": uses,
                "expiresAt": expires_at,
                "expiresAtSecs": expires_at_secs,
                "createdBy": created_by,
                "createdAt": created_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        token = d.pop("token")

        invite_key = d.pop("inviteKey")

        uses = d.pop("uses")

        expires_at = d.pop("expiresAt")

        expires_at_secs = d.pop("expiresAtSecs")

        created_by = d.pop("createdBy")

        created_at = d.pop("createdAt")

        sign_up_invite_create_response_200 = cls(
            token=token,
            invite_key=invite_key,
            uses=uses,
            expires_at=expires_at,
            expires_at_secs=expires_at_secs,
            created_by=created_by,
            created_at=created_at,
        )

        sign_up_invite_create_response_200.additional_properties = d
        return sign_up_invite_create_response_200

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
