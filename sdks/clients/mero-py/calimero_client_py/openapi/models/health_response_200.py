from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="HealthResponse200")


@_attrs_define
class HealthResponse200:
    """
    Attributes:
        ok (bool):
        node (int):
        system_leader (Union[None, int]):
        release (str):
        cluster_version (str):
        rollback_epoch (Union[None, int]):
    """

    ok: bool
    node: int
    system_leader: Union[None, int]
    release: str
    cluster_version: str
    rollback_epoch: Union[None, int]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        ok = self.ok

        node = self.node

        system_leader: Union[None, int]
        system_leader = self.system_leader

        release = self.release

        cluster_version = self.cluster_version

        rollback_epoch: Union[None, int]
        rollback_epoch = self.rollback_epoch

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "ok": ok,
                "node": node,
                "systemLeader": system_leader,
                "release": release,
                "clusterVersion": cluster_version,
                "rollbackEpoch": rollback_epoch,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        ok = d.pop("ok")

        node = d.pop("node")

        def _parse_system_leader(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        system_leader = _parse_system_leader(d.pop("systemLeader"))

        release = d.pop("release")

        cluster_version = d.pop("clusterVersion")

        def _parse_rollback_epoch(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        rollback_epoch = _parse_rollback_epoch(d.pop("rollbackEpoch"))

        health_response_200 = cls(
            ok=ok,
            node=node,
            system_leader=system_leader,
            release=release,
            cluster_version=cluster_version,
            rollback_epoch=rollback_epoch,
        )

        health_response_200.additional_properties = d
        return health_response_200

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
