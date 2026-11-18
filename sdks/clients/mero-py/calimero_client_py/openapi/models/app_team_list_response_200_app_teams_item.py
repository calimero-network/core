from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.team_join import TeamJoin
from typing import cast, Union

T = TypeVar("T", bound="AppTeamListResponse200AppTeamsItem")


@_attrs_define
class AppTeamListResponse200AppTeamsItem:
    """
    Attributes:
        package (str):
        team_id (str):
        name (str):
        join (TeamJoin): Open (`everyone` who reaches the parent) or restricted (`invite`) membership.
        member_count (int):
        default_capabilities (Union[None, int]):
        member (bool):
        manages (bool):
    """

    package: str
    team_id: str
    name: str
    join: TeamJoin
    member_count: int
    default_capabilities: Union[None, int]
    member: bool
    manages: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        package = self.package

        team_id = self.team_id

        name = self.name

        join = self.join.value

        member_count = self.member_count

        default_capabilities: Union[None, int]
        default_capabilities = self.default_capabilities

        member = self.member

        manages = self.manages

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "teamId": team_id,
                "name": name,
                "join": join,
                "memberCount": member_count,
                "defaultCapabilities": default_capabilities,
                "member": member,
                "manages": manages,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        package = d.pop("package")

        team_id = d.pop("teamId")

        name = d.pop("name")

        join = TeamJoin(d.pop("join"))

        member_count = d.pop("memberCount")

        def _parse_default_capabilities(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        default_capabilities = _parse_default_capabilities(d.pop("defaultCapabilities"))

        member = d.pop("member")

        manages = d.pop("manages")

        app_team_list_response_200_app_teams_item = cls(
            package=package,
            team_id=team_id,
            name=name,
            join=join,
            member_count=member_count,
            default_capabilities=default_capabilities,
            member=member,
            manages=manages,
        )

        app_team_list_response_200_app_teams_item.additional_properties = d
        return app_team_list_response_200_app_teams_item

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
