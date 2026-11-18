from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.team_role import TeamRole
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="MemberSetTeamsBodyTeamsItem")


@_attrs_define
class MemberSetTeamsBodyTeamsItem:
    """
    Attributes:
        team (str):
        role (Union[Unset, TeamRole]): A member's standing in one team.
        capabilities (Union[Unset, int]):
    """

    team: str
    role: Union[Unset, TeamRole] = UNSET
    capabilities: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        team = self.team

        role: Union[Unset, str] = UNSET
        if not isinstance(self.role, Unset):
            role = self.role.value

        capabilities = self.capabilities

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "team": team,
            }
        )
        if role is not UNSET:
            field_dict["role"] = role
        if capabilities is not UNSET:
            field_dict["capabilities"] = capabilities

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        team = d.pop("team")

        _role = d.pop("role", UNSET)
        role: Union[Unset, TeamRole]
        if isinstance(_role, Unset):
            role = UNSET
        else:
            role = TeamRole(_role)

        capabilities = d.pop("capabilities", UNSET)

        member_set_teams_body_teams_item = cls(
            team=team,
            role=role,
            capabilities=capabilities,
        )

        member_set_teams_body_teams_item.additional_properties = d
        return member_set_teams_body_teams_item

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
