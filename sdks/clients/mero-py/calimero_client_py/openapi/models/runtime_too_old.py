from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.runtime_too_old_nodes_item import RuntimeTooOldNodesItem


T = TypeVar("T", bound="RuntimeTooOld")


@_attrs_define
class RuntimeTooOld:
    """`details` of a `409 RUNTIME_TOO_OLD`: the runtime the release needs and the replicas known to fall short of it, each
    with what it runs (a version, or `{ before }`: a release older than that one).

        Attributes:
            min_runtime_version (str):
            nodes (list['RuntimeTooOldNodesItem']):
    """

    min_runtime_version: str
    nodes: list["RuntimeTooOldNodesItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.runtime_too_old_nodes_item import RuntimeTooOldNodesItem

        min_runtime_version = self.min_runtime_version

        nodes = []
        for nodes_item_data in self.nodes:
            nodes_item = nodes_item_data.to_dict()
            nodes.append(nodes_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "minRuntimeVersion": min_runtime_version,
                "nodes": nodes,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.runtime_too_old_nodes_item import RuntimeTooOldNodesItem

        d = dict(src_dict)
        min_runtime_version = d.pop("minRuntimeVersion")

        nodes = []
        _nodes = d.pop("nodes")
        for nodes_item_data in _nodes:
            nodes_item = RuntimeTooOldNodesItem.from_dict(nodes_item_data)

            nodes.append(nodes_item)

        runtime_too_old = cls(
            min_runtime_version=min_runtime_version,
            nodes=nodes,
        )

        runtime_too_old.additional_properties = d
        return runtime_too_old

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
