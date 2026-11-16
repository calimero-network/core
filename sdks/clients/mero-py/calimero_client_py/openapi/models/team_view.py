from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.team_join import TeamJoin
from ..models.team_role import TeamRole
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.team_view_apps_item import TeamViewAppsItem
    from ..models.team_view_members_item import TeamViewMembersItem


T = TypeVar("T", bound="TeamView")


@_attrs_define
class TeamView:
    """A team as `GET /orgs/{org}/teams/{team}` answers it: each member's row, the child teams the caller sees, its apps,
    and the caller's own row (`myRole`, `null` for none) and membership (`member`). `package` names the package of an
    app team; it and `defaultCapabilities` are absent from an older node (1.0.0-rc.1 sends neither).

        Attributes:
            team_id (str):
            name (str):
            parent (Union[None, str]):
            join (TeamJoin): Open (`everyone` who reaches the parent) or restricted (`invite`) membership.
            metadata (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
            members (list['TeamViewMembersItem']):
            children (list[str]):
            apps (list['TeamViewAppsItem']):
            my_role (Union[None, TeamRole]):
            member (bool):
            package (Union[None, Unset, str]):
            default_capabilities (Union[None, Unset, int]):
    """

    team_id: str
    name: str
    parent: Union[None, str]
    join: TeamJoin
    metadata: Any
    members: list["TeamViewMembersItem"]
    children: list[str]
    apps: list["TeamViewAppsItem"]
    my_role: Union[None, TeamRole]
    member: bool
    package: Union[None, Unset, str] = UNSET
    default_capabilities: Union[None, Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.team_view_apps_item import TeamViewAppsItem
        from ..models.team_view_members_item import TeamViewMembersItem

        team_id = self.team_id

        name = self.name

        parent: Union[None, str]
        parent = self.parent

        join = self.join.value

        metadata = self.metadata

        members = []
        for members_item_data in self.members:
            members_item = members_item_data.to_dict()
            members.append(members_item)

        children = self.children

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        my_role: Union[None, str]
        if isinstance(self.my_role, TeamRole):
            my_role = self.my_role.value
        else:
            my_role = self.my_role

        member = self.member

        package: Union[None, Unset, str]
        if isinstance(self.package, Unset):
            package = UNSET
        else:
            package = self.package

        default_capabilities: Union[None, Unset, int]
        if isinstance(self.default_capabilities, Unset):
            default_capabilities = UNSET
        else:
            default_capabilities = self.default_capabilities

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "teamId": team_id,
                "name": name,
                "parent": parent,
                "join": join,
                "metadata": metadata,
                "members": members,
                "children": children,
                "apps": apps,
                "myRole": my_role,
                "member": member,
            }
        )
        if package is not UNSET:
            field_dict["package"] = package
        if default_capabilities is not UNSET:
            field_dict["defaultCapabilities"] = default_capabilities

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.team_view_apps_item import TeamViewAppsItem
        from ..models.team_view_members_item import TeamViewMembersItem

        d = dict(src_dict)
        team_id = d.pop("teamId")

        name = d.pop("name")

        def _parse_parent(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        parent = _parse_parent(d.pop("parent"))

        join = TeamJoin(d.pop("join"))

        metadata = d.pop("metadata")

        members = []
        _members = d.pop("members")
        for members_item_data in _members:
            members_item = TeamViewMembersItem.from_dict(members_item_data)

            members.append(members_item)

        children = cast(list[str], d.pop("children"))

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = TeamViewAppsItem.from_dict(apps_item_data)

            apps.append(apps_item)

        def _parse_my_role(data: object) -> Union[None, TeamRole]:
            if data is None:
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                my_role_type_0 = TeamRole(data)

                return my_role_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, TeamRole], data)

        my_role = _parse_my_role(d.pop("myRole"))

        member = d.pop("member")

        def _parse_package(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        package = _parse_package(d.pop("package", UNSET))

        def _parse_default_capabilities(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        default_capabilities = _parse_default_capabilities(
            d.pop("defaultCapabilities", UNSET)
        )

        team_view = cls(
            team_id=team_id,
            name=name,
            parent=parent,
            join=join,
            metadata=metadata,
            members=members,
            children=children,
            apps=apps,
            my_role=my_role,
            member=member,
            package=package,
            default_capabilities=default_capabilities,
        )

        team_view.additional_properties = d
        return team_view

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
