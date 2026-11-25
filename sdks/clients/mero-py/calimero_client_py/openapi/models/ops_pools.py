from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.ops_pools_pools import OpsPoolsPools
    from ..models.ops_pools_capacity import OpsPoolsCapacity


T = TypeVar("T", bound="OpsPools")


@_attrs_define
class OpsPools:
    """`GET /ops/pools`: the pools, and the runs each takes at once.

    Attributes:
        pools (OpsPoolsPools):
        capacity (OpsPoolsCapacity):
    """

    pools: "OpsPoolsPools"
    capacity: "OpsPoolsCapacity"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.ops_pools_pools import OpsPoolsPools
        from ..models.ops_pools_capacity import OpsPoolsCapacity

        pools = self.pools.to_dict()

        capacity = self.capacity.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "pools": pools,
                "capacity": capacity,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.ops_pools_pools import OpsPoolsPools
        from ..models.ops_pools_capacity import OpsPoolsCapacity

        d = dict(src_dict)
        pools = OpsPoolsPools.from_dict(d.pop("pools"))

        capacity = OpsPoolsCapacity.from_dict(d.pop("capacity"))

        ops_pools = cls(
            pools=pools,
            capacity=capacity,
        )

        ops_pools.additional_properties = d
        return ops_pools

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
