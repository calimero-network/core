from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_team_list_response_200_mode import AppTeamListResponse200Mode
from typing import cast

if TYPE_CHECKING:
    from ..models.app_team_list_response_200_app_teams_item import (
        AppTeamListResponse200AppTeamsItem,
    )


T = TypeVar("T", bound="AppTeamListResponse200")


@_attrs_define
class AppTeamListResponse200:
    """
    Attributes:
        mode (AppTeamListResponse200Mode):
        app_teams (list['AppTeamListResponse200AppTeamsItem']):
    """

    mode: AppTeamListResponse200Mode
    app_teams: list["AppTeamListResponse200AppTeamsItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_team_list_response_200_app_teams_item import (
            AppTeamListResponse200AppTeamsItem,
        )

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
        from ..models.app_team_list_response_200_app_teams_item import (
            AppTeamListResponse200AppTeamsItem,
        )

        d = dict(src_dict)
        mode = AppTeamListResponse200Mode(d.pop("mode"))

        app_teams = []
        _app_teams = d.pop("appTeams")
        for app_teams_item_data in _app_teams:
            app_teams_item = AppTeamListResponse200AppTeamsItem.from_dict(
                app_teams_item_data
            )

            app_teams.append(app_teams_item)

        app_team_list_response_200 = cls(
            mode=mode,
            app_teams=app_teams,
        )

        app_team_list_response_200.additional_properties = d
        return app_team_list_response_200

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
