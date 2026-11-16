from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.team_join import TeamJoin
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.team_members import TeamMembers
    from ..models.team_capabilities import TeamCapabilities


T = TypeVar("T", bound="Team")


@_attrs_define
class Team:
    """A team as its organization records it: `members` maps each direct row to whether it is a lead; `capabilities` are
    capability bits per row; `defaultCapabilities` is absent when unset.

        Attributes:
            team_id (str):
            name (str):
            parent (Union[None, str]):
            join (TeamJoin): Open (`everyone` who reaches the parent) or restricted (`invite`) membership.
            members (TeamMembers):
            read_only (list[str]):
            capabilities (TeamCapabilities):
            metadata (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
            removed (list[str]):
            default_capabilities (Union[Unset, int]):
    """

    team_id: str
    name: str
    parent: Union[None, str]
    join: TeamJoin
    members: "TeamMembers"
    read_only: list[str]
    capabilities: "TeamCapabilities"
    metadata: Any
    removed: list[str]
    default_capabilities: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.team_members import TeamMembers
        from ..models.team_capabilities import TeamCapabilities

        team_id = self.team_id

        name = self.name

        parent: Union[None, str]
        parent = self.parent

        join = self.join.value

        members = self.members.to_dict()

        read_only = self.read_only

        capabilities = self.capabilities.to_dict()

        metadata = self.metadata

        removed = self.removed

        default_capabilities = self.default_capabilities

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "teamId": team_id,
                "name": name,
                "parent": parent,
                "join": join,
                "members": members,
                "readOnly": read_only,
                "capabilities": capabilities,
                "metadata": metadata,
                "removed": removed,
            }
        )
        if default_capabilities is not UNSET:
            field_dict["defaultCapabilities"] = default_capabilities

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.team_members import TeamMembers
        from ..models.team_capabilities import TeamCapabilities

        d = dict(src_dict)
        team_id = d.pop("teamId")

        name = d.pop("name")

        def _parse_parent(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        parent = _parse_parent(d.pop("parent"))

        join = TeamJoin(d.pop("join"))

        members = TeamMembers.from_dict(d.pop("members"))

        read_only = cast(list[str], d.pop("readOnly"))

        capabilities = TeamCapabilities.from_dict(d.pop("capabilities"))

        metadata = d.pop("metadata")

        removed = cast(list[str], d.pop("removed"))

        default_capabilities = d.pop("defaultCapabilities", UNSET)

        team = cls(
            team_id=team_id,
            name=name,
            parent=parent,
            join=join,
            members=members,
            read_only=read_only,
            capabilities=capabilities,
            metadata=metadata,
            removed=removed,
            default_capabilities=default_capabilities,
        )

        team.additional_properties = d
        return team

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
