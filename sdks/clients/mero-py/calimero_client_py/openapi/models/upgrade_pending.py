from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.upgrade_pending_nodes_item import UpgradePendingNodesItem


T = TypeVar("T", bound="UpgradePending")


@_attrs_define
class UpgradePending:
    """`details` of a `409 UPGRADE_PENDING`: the release every node must run first, and the nodes that run an older one
    (`release` `null`: one this node could not attest).

        Attributes:
            release (str):
            nodes (list['UpgradePendingNodesItem']):
    """

    release: str
    nodes: list["UpgradePendingNodesItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.upgrade_pending_nodes_item import UpgradePendingNodesItem

        release = self.release

        nodes = []
        for nodes_item_data in self.nodes:
            nodes_item = nodes_item_data.to_dict()
            nodes.append(nodes_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "release": release,
                "nodes": nodes,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.upgrade_pending_nodes_item import UpgradePendingNodesItem

        d = dict(src_dict)
        release = d.pop("release")

        nodes = []
        _nodes = d.pop("nodes")
        for nodes_item_data in _nodes:
            nodes_item = UpgradePendingNodesItem.from_dict(nodes_item_data)

            nodes.append(nodes_item)

        upgrade_pending = cls(
            release=release,
            nodes=nodes,
        )

        upgrade_pending.additional_properties = d
        return upgrade_pending

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
