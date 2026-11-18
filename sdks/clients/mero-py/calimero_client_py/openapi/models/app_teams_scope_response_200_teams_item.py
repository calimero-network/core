from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.team_join import TeamJoin
from typing import cast
from typing import cast, Union

T = TypeVar("T", bound="AppTeamsScopeResponse200TeamsItem")


@_attrs_define
class AppTeamsScopeResponse200TeamsItem:
    """
    Attributes:
        package (str):
        team_id (str):
        created (bool):
        apps (list[str]):
        added (int):
        join (Union[None, TeamJoin]):
        dropped (int):
    """

    package: str
    team_id: str
    created: bool
    apps: list[str]
    added: int
    join: Union[None, TeamJoin]
    dropped: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        package = self.package

        team_id = self.team_id

        created = self.created

        apps = self.apps

        added = self.added

        join: Union[None, str]
        if isinstance(self.join, TeamJoin):
            join = self.join.value
        else:
            join = self.join

        dropped = self.dropped

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "teamId": team_id,
                "created": created,
                "apps": apps,
                "added": added,
                "join": join,
                "dropped": dropped,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        package = d.pop("package")

        team_id = d.pop("teamId")

        created = d.pop("created")

        apps = cast(list[str], d.pop("apps"))

        added = d.pop("added")

        def _parse_join(data: object) -> Union[None, TeamJoin]:
            if data is None:
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                join_type_0 = TeamJoin(data)

                return join_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, TeamJoin], data)

        join = _parse_join(d.pop("join"))

        dropped = d.pop("dropped")

        app_teams_scope_response_200_teams_item = cls(
            package=package,
            team_id=team_id,
            created=created,
            apps=apps,
            added=added,
            join=join,
            dropped=dropped,
        )

        app_teams_scope_response_200_teams_item.additional_properties = d
        return app_teams_scope_response_200_teams_item

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
