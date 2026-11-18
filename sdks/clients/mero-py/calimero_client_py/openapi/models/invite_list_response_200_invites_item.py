from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.org_role import OrgRole
from typing import cast
from typing import cast, Union

T = TypeVar("T", bound="InviteListResponse200InvitesItem")


@_attrs_define
class InviteListResponse200InvitesItem:
    """
    Attributes:
        invite_key (str):
        role (OrgRole): A member's role in an organization.
        teams (list[str]):
        uses_left (int):
        expires_at (int):
        expired (bool):
        created_by (Union[None, str]):
        created_at (Union[None, int]):
        all_apps (bool):
    """

    invite_key: str
    role: OrgRole
    teams: list[str]
    uses_left: int
    expires_at: int
    expired: bool
    created_by: Union[None, str]
    created_at: Union[None, int]
    all_apps: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        invite_key = self.invite_key

        role = self.role.value

        teams = self.teams

        uses_left = self.uses_left

        expires_at = self.expires_at

        expired = self.expired

        created_by: Union[None, str]
        created_by = self.created_by

        created_at: Union[None, int]
        created_at = self.created_at

        all_apps = self.all_apps

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "inviteKey": invite_key,
                "role": role,
                "teams": teams,
                "usesLeft": uses_left,
                "expiresAt": expires_at,
                "expired": expired,
                "createdBy": created_by,
                "createdAt": created_at,
                "allApps": all_apps,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        invite_key = d.pop("inviteKey")

        role = OrgRole(d.pop("role"))

        teams = cast(list[str], d.pop("teams"))

        uses_left = d.pop("usesLeft")

        expires_at = d.pop("expiresAt")

        expired = d.pop("expired")

        def _parse_created_by(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        created_by = _parse_created_by(d.pop("createdBy"))

        def _parse_created_at(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        created_at = _parse_created_at(d.pop("createdAt"))

        all_apps = d.pop("allApps")

        invite_list_response_200_invites_item = cls(
            invite_key=invite_key,
            role=role,
            teams=teams,
            uses_left=uses_left,
            expires_at=expires_at,
            expired=expired,
            created_by=created_by,
            created_at=created_at,
            all_apps=all_apps,
        )

        invite_list_response_200_invites_item.additional_properties = d
        return invite_list_response_200_invites_item

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
