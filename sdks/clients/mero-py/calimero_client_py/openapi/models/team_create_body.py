from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.team_join import TeamJoin
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="TeamCreateBody")


@_attrs_define
class TeamCreateBody:
    """
    Attributes:
        name (str):
        join (TeamJoin): Open (`everyone` who reaches the parent) or restricted (`invite`) membership.
        parent (Union[Unset, str]):
    """

    name: str
    join: TeamJoin
    parent: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        name = self.name

        join = self.join.value

        parent = self.parent

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "name": name,
                "join": join,
            }
        )
        if parent is not UNSET:
            field_dict["parent"] = parent

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        name = d.pop("name")

        join = TeamJoin(d.pop("join"))

        parent = d.pop("parent", UNSET)

        team_create_body = cls(
            name=name,
            join=join,
            parent=parent,
        )

        team_create_body.additional_properties = d
        return team_create_body

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
