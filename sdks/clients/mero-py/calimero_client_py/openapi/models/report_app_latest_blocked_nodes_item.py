from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.report_app_latest_blocked_nodes_item_runtime_type_1 import (
        ReportAppLatestBlockedNodesItemRuntimeType1,
    )


T = TypeVar("T", bound="ReportAppLatestBlockedNodesItem")


@_attrs_define
class ReportAppLatestBlockedNodesItem:
    """
    Attributes:
        node (int):
        runtime (Union['ReportAppLatestBlockedNodesItemRuntimeType1', None, str]):
    """

    node: int
    runtime: Union["ReportAppLatestBlockedNodesItemRuntimeType1", None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.report_app_latest_blocked_nodes_item_runtime_type_1 import (
            ReportAppLatestBlockedNodesItemRuntimeType1,
        )

        node = self.node

        runtime: Union[None, dict[str, Any], str]
        if isinstance(self.runtime, ReportAppLatestBlockedNodesItemRuntimeType1):
            runtime = self.runtime.to_dict()
        else:
            runtime = self.runtime

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "node": node,
                "runtime": runtime,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.report_app_latest_blocked_nodes_item_runtime_type_1 import (
            ReportAppLatestBlockedNodesItemRuntimeType1,
        )

        d = dict(src_dict)
        node = d.pop("node")

        def _parse_runtime(
            data: object,
        ) -> Union["ReportAppLatestBlockedNodesItemRuntimeType1", None, str]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                runtime_type_1 = ReportAppLatestBlockedNodesItemRuntimeType1.from_dict(
                    data
                )

                return runtime_type_1
            except:  # noqa: E722
                pass
            return cast(
                Union["ReportAppLatestBlockedNodesItemRuntimeType1", None, str], data
            )

        runtime = _parse_runtime(d.pop("runtime"))

        report_app_latest_blocked_nodes_item = cls(
            node=node,
            runtime=runtime,
        )

        report_app_latest_blocked_nodes_item.additional_properties = d
        return report_app_latest_blocked_nodes_item

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
