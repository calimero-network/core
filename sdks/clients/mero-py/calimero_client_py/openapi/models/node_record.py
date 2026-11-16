from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.node_role import NodeRole
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="NodeRecord")


@_attrs_define
class NodeRecord:
    """A registered node: its id, address, zone, role and (a job node's) compute pool, absent for none.

    Attributes:
        id (int):
        addr (str):
        zone (str):
        role (NodeRole): What a node does: hold app replicas, or run compute jobs.
        pool (Union[Unset, str]):
    """

    id: int
    addr: str
    zone: str
    role: NodeRole
    pool: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        addr = self.addr

        zone = self.zone

        role = self.role.value

        pool = self.pool

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "addr": addr,
                "zone": zone,
                "role": role,
            }
        )
        if pool is not UNSET:
            field_dict["pool"] = pool

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        id = d.pop("id")

        addr = d.pop("addr")

        zone = d.pop("zone")

        role = NodeRole(d.pop("role"))

        pool = d.pop("pool", UNSET)

        node_record = cls(
            id=id,
            addr=addr,
            zone=zone,
            role=role,
            pool=pool,
        )

        node_record.additional_properties = d
        return node_record

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
