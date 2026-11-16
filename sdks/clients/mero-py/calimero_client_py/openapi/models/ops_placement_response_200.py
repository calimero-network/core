from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.ops_placement_response_200_apps_item import (
        OpsPlacementResponse200AppsItem,
    )
    from ..models.ops_placement_response_200_nodes_item import (
        OpsPlacementResponse200NodesItem,
    )


T = TypeVar("T", bound="OpsPlacementResponse200")


@_attrs_define
class OpsPlacementResponse200:
    """
    Attributes:
        fresh (bool):
        apps (list['OpsPlacementResponse200AppsItem']):
        nodes (list['OpsPlacementResponse200NodesItem']):
        cluster_version (Union[None, str]):
    """

    fresh: bool
    apps: list["OpsPlacementResponse200AppsItem"]
    nodes: list["OpsPlacementResponse200NodesItem"]
    cluster_version: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.ops_placement_response_200_apps_item import (
            OpsPlacementResponse200AppsItem,
        )
        from ..models.ops_placement_response_200_nodes_item import (
            OpsPlacementResponse200NodesItem,
        )

        fresh = self.fresh

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        nodes = []
        for nodes_item_data in self.nodes:
            nodes_item = nodes_item_data.to_dict()
            nodes.append(nodes_item)

        cluster_version: Union[None, str]
        cluster_version = self.cluster_version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "fresh": fresh,
                "apps": apps,
                "nodes": nodes,
                "clusterVersion": cluster_version,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.ops_placement_response_200_apps_item import (
            OpsPlacementResponse200AppsItem,
        )
        from ..models.ops_placement_response_200_nodes_item import (
            OpsPlacementResponse200NodesItem,
        )

        d = dict(src_dict)
        fresh = d.pop("fresh")

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = OpsPlacementResponse200AppsItem.from_dict(apps_item_data)

            apps.append(apps_item)

        nodes = []
        _nodes = d.pop("nodes")
        for nodes_item_data in _nodes:
            nodes_item = OpsPlacementResponse200NodesItem.from_dict(nodes_item_data)

            nodes.append(nodes_item)

        def _parse_cluster_version(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        cluster_version = _parse_cluster_version(d.pop("clusterVersion"))

        ops_placement_response_200 = cls(
            fresh=fresh,
            apps=apps,
            nodes=nodes,
            cluster_version=cluster_version,
        )

        ops_placement_response_200.additional_properties = d
        return ops_placement_response_200

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
