from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="OrgQuotas")


@_attrs_define
class OrgQuotas:
    """One payer's limits in place of the cluster's; a limit left out is the cluster's.

    Attributes:
        max_concurrent_runs (Union[Unset, int]):
        compute_fuel (Union[Unset, str]): an unsigned 64-bit integer as a decimal string (D20)
        max_file_bytes (Union[Unset, int]):
    """

    max_concurrent_runs: Union[Unset, int] = UNSET
    compute_fuel: Union[Unset, str] = UNSET
    max_file_bytes: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        max_concurrent_runs = self.max_concurrent_runs

        compute_fuel = self.compute_fuel

        max_file_bytes = self.max_file_bytes

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if max_concurrent_runs is not UNSET:
            field_dict["maxConcurrentRuns"] = max_concurrent_runs
        if compute_fuel is not UNSET:
            field_dict["computeFuel"] = compute_fuel
        if max_file_bytes is not UNSET:
            field_dict["maxFileBytes"] = max_file_bytes

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        max_concurrent_runs = d.pop("maxConcurrentRuns", UNSET)

        compute_fuel = d.pop("computeFuel", UNSET)

        max_file_bytes = d.pop("maxFileBytes", UNSET)

        org_quotas = cls(
            max_concurrent_runs=max_concurrent_runs,
            compute_fuel=compute_fuel,
            max_file_bytes=max_file_bytes,
        )

        org_quotas.additional_properties = d
        return org_quotas

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
