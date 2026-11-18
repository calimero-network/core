from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.team_join import TeamJoin
from ..types import UNSET, Unset
from typing import cast
from typing import Union

T = TypeVar("T", bound="AppTeamsScopeBodyAppsItem")


@_attrs_define
class AppTeamsScopeBodyAppsItem:
    """
    Attributes:
        package (str):
        name (Union[Unset, str]):
        instances (Union[Unset, list[str]]):
        join (Union[Unset, TeamJoin]): Open (`everyone` who reaches the parent) or restricted (`invite`) membership.
        keep (Union[Unset, list[str]]):
        child_teams (Union[Unset, bool]):
    """

    package: str
    name: Union[Unset, str] = UNSET
    instances: Union[Unset, list[str]] = UNSET
    join: Union[Unset, TeamJoin] = UNSET
    keep: Union[Unset, list[str]] = UNSET
    child_teams: Union[Unset, bool] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        package = self.package

        name = self.name

        instances: Union[Unset, list[str]] = UNSET
        if not isinstance(self.instances, Unset):
            instances = self.instances

        join: Union[Unset, str] = UNSET
        if not isinstance(self.join, Unset):
            join = self.join.value

        keep: Union[Unset, list[str]] = UNSET
        if not isinstance(self.keep, Unset):
            keep = self.keep

        child_teams = self.child_teams

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
            }
        )
        if name is not UNSET:
            field_dict["name"] = name
        if instances is not UNSET:
            field_dict["instances"] = instances
        if join is not UNSET:
            field_dict["join"] = join
        if keep is not UNSET:
            field_dict["keep"] = keep
        if child_teams is not UNSET:
            field_dict["childTeams"] = child_teams

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        package = d.pop("package")

        name = d.pop("name", UNSET)

        instances = cast(list[str], d.pop("instances", UNSET))

        _join = d.pop("join", UNSET)
        join: Union[Unset, TeamJoin]
        if isinstance(_join, Unset):
            join = UNSET
        else:
            join = TeamJoin(_join)

        keep = cast(list[str], d.pop("keep", UNSET))

        child_teams = d.pop("childTeams", UNSET)

        app_teams_scope_body_apps_item = cls(
            package=package,
            name=name,
            instances=instances,
            join=join,
            keep=keep,
            child_teams=child_teams,
        )

        app_teams_scope_body_apps_item.additional_properties = d
        return app_teams_scope_body_apps_item

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
