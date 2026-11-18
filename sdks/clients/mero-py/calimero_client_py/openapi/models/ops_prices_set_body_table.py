from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="OpsPricesSetBodyTable")


@_attrs_define
class OpsPricesSetBodyTable:
    """
    Attributes:
        version (int):
        fuel_per_vcpu_second (int):
        vcpu_second (int):
        gib_second (int):
        connector_gib (int):
        storage_gib_month (int):
        egress_gib (int):
    """

    version: int
    fuel_per_vcpu_second: int
    vcpu_second: int
    gib_second: int
    connector_gib: int
    storage_gib_month: int
    egress_gib: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        version = self.version

        fuel_per_vcpu_second = self.fuel_per_vcpu_second

        vcpu_second = self.vcpu_second

        gib_second = self.gib_second

        connector_gib = self.connector_gib

        storage_gib_month = self.storage_gib_month

        egress_gib = self.egress_gib

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "version": version,
                "fuelPerVcpuSecond": fuel_per_vcpu_second,
                "vcpuSecond": vcpu_second,
                "gibSecond": gib_second,
                "connectorGib": connector_gib,
                "storageGibMonth": storage_gib_month,
                "egressGib": egress_gib,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        version = d.pop("version")

        fuel_per_vcpu_second = d.pop("fuelPerVcpuSecond")

        vcpu_second = d.pop("vcpuSecond")

        gib_second = d.pop("gibSecond")

        connector_gib = d.pop("connectorGib")

        storage_gib_month = d.pop("storageGibMonth")

        egress_gib = d.pop("egressGib")

        ops_prices_set_body_table = cls(
            version=version,
            fuel_per_vcpu_second=fuel_per_vcpu_second,
            vcpu_second=vcpu_second,
            gib_second=gib_second,
            connector_gib=connector_gib,
            storage_gib_month=storage_gib_month,
            egress_gib=egress_gib,
        )

        ops_prices_set_body_table.additional_properties = d
        return ops_prices_set_body_table

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
