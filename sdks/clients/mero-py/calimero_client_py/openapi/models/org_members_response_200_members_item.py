from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.org_members_response_200_members_item_passkeys_type_1 import (
    OrgMembersResponse200MembersItemPasskeysType1,
)
from ..models.org_role import OrgRole
from typing import cast
from typing import cast, Union

T = TypeVar("T", bound="OrgMembersResponse200MembersItem")


@_attrs_define
class OrgMembersResponse200MembersItem:
    """
    Attributes:
        account (str):
        name (Union[None, str]):
        role (OrgRole): A member's role in an organization.
        teams (list[str]):
        passkeys (Union[OrgMembersResponse200MembersItemPasskeysType1, int]):
        recovery_code (bool):
        last_active_day (Union[None, str]):
        sessions (int):
    """

    account: str
    name: Union[None, str]
    role: OrgRole
    teams: list[str]
    passkeys: Union[OrgMembersResponse200MembersItemPasskeysType1, int]
    recovery_code: bool
    last_active_day: Union[None, str]
    sessions: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        account = self.account

        name: Union[None, str]
        name = self.name

        role = self.role.value

        teams = self.teams

        passkeys: Union[int, str]
        if isinstance(self.passkeys, OrgMembersResponse200MembersItemPasskeysType1):
            passkeys = self.passkeys.value
        else:
            passkeys = self.passkeys

        recovery_code = self.recovery_code

        last_active_day: Union[None, str]
        last_active_day = self.last_active_day

        sessions = self.sessions

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "account": account,
                "name": name,
                "role": role,
                "teams": teams,
                "passkeys": passkeys,
                "recoveryCode": recovery_code,
                "lastActiveDay": last_active_day,
                "sessions": sessions,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        account = d.pop("account")

        def _parse_name(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        name = _parse_name(d.pop("name"))

        role = OrgRole(d.pop("role"))

        teams = cast(list[str], d.pop("teams"))

        def _parse_passkeys(
            data: object,
        ) -> Union[OrgMembersResponse200MembersItemPasskeysType1, int]:
            try:
                if not isinstance(data, str):
                    raise TypeError()
                passkeys_type_1 = OrgMembersResponse200MembersItemPasskeysType1(data)

                return passkeys_type_1
            except:  # noqa: E722
                pass
            return cast(Union[OrgMembersResponse200MembersItemPasskeysType1, int], data)

        passkeys = _parse_passkeys(d.pop("passkeys"))

        recovery_code = d.pop("recoveryCode")

        def _parse_last_active_day(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        last_active_day = _parse_last_active_day(d.pop("lastActiveDay"))

        sessions = d.pop("sessions")

        org_members_response_200_members_item = cls(
            account=account,
            name=name,
            role=role,
            teams=teams,
            passkeys=passkeys,
            recovery_code=recovery_code,
            last_active_day=last_active_day,
            sessions=sessions,
        )

        org_members_response_200_members_item.additional_properties = d
        return org_members_response_200_members_item

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
