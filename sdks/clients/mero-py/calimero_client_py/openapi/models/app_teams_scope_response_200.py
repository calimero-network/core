from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_teams_scope_response_200_mode import AppTeamsScopeResponse200Mode
from typing import cast

if TYPE_CHECKING:
    from ..models.app_teams_scope_response_200_teams_item import (
        AppTeamsScopeResponse200TeamsItem,
    )


T = TypeVar("T", bound="AppTeamsScopeResponse200")


@_attrs_define
class AppTeamsScopeResponse200:
    """
    Attributes:
        mode (AppTeamsScopeResponse200Mode):
        teams (list['AppTeamsScopeResponse200TeamsItem']):
    """

    mode: AppTeamsScopeResponse200Mode
    teams: list["AppTeamsScopeResponse200TeamsItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_teams_scope_response_200_teams_item import (
            AppTeamsScopeResponse200TeamsItem,
        )

        mode = self.mode.value

        teams = []
        for teams_item_data in self.teams:
            teams_item = teams_item_data.to_dict()
            teams.append(teams_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "mode": mode,
                "teams": teams,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_teams_scope_response_200_teams_item import (
            AppTeamsScopeResponse200TeamsItem,
        )

        d = dict(src_dict)
        mode = AppTeamsScopeResponse200Mode(d.pop("mode"))

        teams = []
        _teams = d.pop("teams")
        for teams_item_data in _teams:
            teams_item = AppTeamsScopeResponse200TeamsItem.from_dict(teams_item_data)

            teams.append(teams_item)

        app_teams_scope_response_200 = cls(
            mode=mode,
            teams=teams,
        )

        app_teams_scope_response_200.additional_properties = d
        return app_teams_scope_response_200

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
