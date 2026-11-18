from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.node_role import NodeRole
from typing import cast, Union

T = TypeVar("T", bound="OpsPlacementResponse200NodesItem")


@_attrs_define
class OpsPlacementResponse200NodesItem:
    """
    Attributes:
        id (int):
        zone (str):
        role (NodeRole): What a node does: hold app replicas, or run compute jobs.
        release (Union[None, str]):
        apps (int):
    """

    id: int
    zone: str
    role: NodeRole
    release: Union[None, str]
    apps: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        zone = self.zone

        role = self.role.value

        release: Union[None, str]
        release = self.release

        apps = self.apps

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "zone": zone,
                "role": role,
                "release": release,
                "apps": apps,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        id = d.pop("id")

        zone = d.pop("zone")

        role = NodeRole(d.pop("role"))

        def _parse_release(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        release = _parse_release(d.pop("release"))

        apps = d.pop("apps")

        ops_placement_response_200_nodes_item = cls(
            id=id,
            zone=zone,
            role=role,
            release=release,
            apps=apps,
        )

        ops_placement_response_200_nodes_item.additional_properties = d
        return ops_placement_response_200_nodes_item

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
