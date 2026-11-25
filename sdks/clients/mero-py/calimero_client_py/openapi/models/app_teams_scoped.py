from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_teams_scoped_mode import AppTeamsScopedMode
from typing import cast

if TYPE_CHECKING:
    from ..models.scoped_team import ScopedTeam


T = TypeVar("T", bound="AppTeamsScoped")


@_attrs_define
class AppTeamsScoped:
    """What `ScopeApps` answers: the organization's mode after, and each package's app team.

    Attributes:
        mode (AppTeamsScopedMode):
        teams (list['ScopedTeam']):
    """

    mode: AppTeamsScopedMode
    teams: list["ScopedTeam"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.scoped_team import ScopedTeam

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
        from ..models.scoped_team import ScopedTeam

        d = dict(src_dict)
        mode = AppTeamsScopedMode(d.pop("mode"))

        teams = []
        _teams = d.pop("teams")
        for teams_item_data in _teams:
            teams_item = ScopedTeam.from_dict(teams_item_data)

            teams.append(teams_item)

        app_teams_scoped = cls(
            mode=mode,
            teams=teams,
        )

        app_teams_scoped.additional_properties = d
        return app_teams_scoped

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
