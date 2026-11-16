from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.report_app_latest_blocked_reason import ReportAppLatestBlockedReason
from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.report_app_latest_blocked_nodes_item import (
        ReportAppLatestBlockedNodesItem,
    )


T = TypeVar("T", bound="ReportAppLatestBlocked")


@_attrs_define
class ReportAppLatestBlocked:
    """
    Attributes:
        version (str):
        reason (ReportAppLatestBlockedReason):
        message (str):
        min_runtime_version (Union[Unset, str]):
        nodes (Union[Unset, list['ReportAppLatestBlockedNodesItem']]):
    """

    version: str
    reason: ReportAppLatestBlockedReason
    message: str
    min_runtime_version: Union[Unset, str] = UNSET
    nodes: Union[Unset, list["ReportAppLatestBlockedNodesItem"]] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.report_app_latest_blocked_nodes_item import (
            ReportAppLatestBlockedNodesItem,
        )

        version = self.version

        reason = self.reason.value

        message = self.message

        min_runtime_version = self.min_runtime_version

        nodes: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.nodes, Unset):
            nodes = []
            for nodes_item_data in self.nodes:
                nodes_item = nodes_item_data.to_dict()
                nodes.append(nodes_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "version": version,
                "reason": reason,
                "message": message,
            }
        )
        if min_runtime_version is not UNSET:
            field_dict["minRuntimeVersion"] = min_runtime_version
        if nodes is not UNSET:
            field_dict["nodes"] = nodes

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.report_app_latest_blocked_nodes_item import (
            ReportAppLatestBlockedNodesItem,
        )

        d = dict(src_dict)
        version = d.pop("version")

        reason = ReportAppLatestBlockedReason(d.pop("reason"))

        message = d.pop("message")

        min_runtime_version = d.pop("minRuntimeVersion", UNSET)

        nodes = []
        _nodes = d.pop("nodes", UNSET)
        for nodes_item_data in _nodes or []:
            nodes_item = ReportAppLatestBlockedNodesItem.from_dict(nodes_item_data)

            nodes.append(nodes_item)

        report_app_latest_blocked = cls(
            version=version,
            reason=reason,
            message=message,
            min_runtime_version=min_runtime_version,
            nodes=nodes,
        )

        report_app_latest_blocked.additional_properties = d
        return report_app_latest_blocked

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
