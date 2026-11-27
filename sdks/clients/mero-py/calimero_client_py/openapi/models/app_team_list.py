from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_team_list_mode import AppTeamListMode
from typing import cast

if TYPE_CHECKING:
    from ..models.app_team_row import AppTeamRow


T = TypeVar("T", bound="AppTeamList")


@_attrs_define
class AppTeamList:
    """`GET /orgs/{org}/app-teams`: the organization's mode and its app teams.

    Attributes:
        mode (AppTeamListMode):
        app_teams (list['AppTeamRow']):
    """

    mode: AppTeamListMode
    app_teams: list["AppTeamRow"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_team_row import AppTeamRow

        mode = self.mode.value

        app_teams = []
        for app_teams_item_data in self.app_teams:
            app_teams_item = app_teams_item_data.to_dict()
            app_teams.append(app_teams_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "mode": mode,
                "appTeams": app_teams,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_team_row import AppTeamRow

        d = dict(src_dict)
        mode = AppTeamListMode(d.pop("mode"))

        app_teams = []
        _app_teams = d.pop("appTeams")
        for app_teams_item_data in _app_teams:
            app_teams_item = AppTeamRow.from_dict(app_teams_item_data)

            app_teams.append(app_teams_item)

        app_team_list = cls(
            mode=mode,
            app_teams=app_teams,
        )

        app_team_list.additional_properties = d
        return app_team_list

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
