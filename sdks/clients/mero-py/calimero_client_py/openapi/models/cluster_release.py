from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ClusterRelease")


@_attrs_define
class ClusterRelease:
    """A mero-tee release the cluster's nodes run: how many run it, and whether the scope approved it (`true` where nothing
    was ever recorded).

        Attributes:
            release (str):
            nodes (int):
            approved (bool):
    """

    release: str
    nodes: int
    approved: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        release = self.release

        nodes = self.nodes

        approved = self.approved

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "release": release,
                "nodes": nodes,
                "approved": approved,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        release = d.pop("release")

        nodes = d.pop("nodes")

        approved = d.pop("approved")

        cluster_release = cls(
            release=release,
            nodes=nodes,
            approved=approved,
        )

        cluster_release.additional_properties = d
        return cluster_release

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
