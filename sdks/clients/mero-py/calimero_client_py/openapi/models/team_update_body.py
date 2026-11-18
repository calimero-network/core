from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.team_join import TeamJoin
from ..types import UNSET, Unset
from typing import cast, Union
from typing import Union

T = TypeVar("T", bound="TeamUpdateBody")


@_attrs_define
class TeamUpdateBody:
    """
    Attributes:
        name (Union[Unset, str]):
        join (Union[Unset, TeamJoin]): Open (`everyone` who reaches the parent) or restricted (`invite`) membership.
        parent (Union[None, Unset, str]):
        metadata (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for
            whose it is).
    """

    name: Union[Unset, str] = UNSET
    join: Union[Unset, TeamJoin] = UNSET
    parent: Union[None, Unset, str] = UNSET
    metadata: Union[Unset, Any] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        name = self.name

        join: Union[Unset, str] = UNSET
        if not isinstance(self.join, Unset):
            join = self.join.value

        parent: Union[None, Unset, str]
        if isinstance(self.parent, Unset):
            parent = UNSET
        else:
            parent = self.parent

        metadata = self.metadata

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if name is not UNSET:
            field_dict["name"] = name
        if join is not UNSET:
            field_dict["join"] = join
        if parent is not UNSET:
            field_dict["parent"] = parent
        if metadata is not UNSET:
            field_dict["metadata"] = metadata

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        name = d.pop("name", UNSET)

        _join = d.pop("join", UNSET)
        join: Union[Unset, TeamJoin]
        if isinstance(_join, Unset):
            join = UNSET
        else:
            join = TeamJoin(_join)

        def _parse_parent(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        parent = _parse_parent(d.pop("parent", UNSET))

        metadata = d.pop("metadata", UNSET)

        team_update_body = cls(
            name=name,
            join=join,
            parent=parent,
            metadata=metadata,
        )

        team_update_body.additional_properties = d
        return team_update_body

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
