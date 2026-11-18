from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.ops_pools_get_response_200_capacity import (
        OpsPoolsGetResponse200Capacity,
    )
    from ..models.ops_pools_get_response_200_pools import OpsPoolsGetResponse200Pools


T = TypeVar("T", bound="OpsPoolsGetResponse200")


@_attrs_define
class OpsPoolsGetResponse200:
    """
    Attributes:
        pools (OpsPoolsGetResponse200Pools):
        capacity (OpsPoolsGetResponse200Capacity):
    """

    pools: "OpsPoolsGetResponse200Pools"
    capacity: "OpsPoolsGetResponse200Capacity"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.ops_pools_get_response_200_capacity import (
            OpsPoolsGetResponse200Capacity,
        )
        from ..models.ops_pools_get_response_200_pools import (
            OpsPoolsGetResponse200Pools,
        )

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
        from ..models.ops_pools_get_response_200_capacity import (
            OpsPoolsGetResponse200Capacity,
        )
        from ..models.ops_pools_get_response_200_pools import (
            OpsPoolsGetResponse200Pools,
        )

        d = dict(src_dict)
        pools = OpsPoolsGetResponse200Pools.from_dict(d.pop("pools"))

        capacity = OpsPoolsGetResponse200Capacity.from_dict(d.pop("capacity"))

        ops_pools_get_response_200 = cls(
            pools=pools,
            capacity=capacity,
        )

        ops_pools_get_response_200.additional_properties = d
        return ops_pools_get_response_200

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
