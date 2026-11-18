from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="OpsPricesGetResponse200")


@_attrs_define
class OpsPricesGetResponse200:
    """
    Attributes:
        version (Union[Unset, int]):
        fuel_per_vcpu_second (Union[Unset, int]):
        vcpu_second (Union[Unset, int]):
        gib_second (Union[Unset, int]):
        connector_gib (Union[Unset, int]):
        storage_gib_month (Union[Unset, int]):
        egress_gib (Union[Unset, int]):
    """

    version: Union[Unset, int] = UNSET
    fuel_per_vcpu_second: Union[Unset, int] = UNSET
    vcpu_second: Union[Unset, int] = UNSET
    gib_second: Union[Unset, int] = UNSET
    connector_gib: Union[Unset, int] = UNSET
    storage_gib_month: Union[Unset, int] = UNSET
    egress_gib: Union[Unset, int] = UNSET
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
        field_dict.update({})
        if version is not UNSET:
            field_dict["version"] = version
        if fuel_per_vcpu_second is not UNSET:
            field_dict["fuelPerVcpuSecond"] = fuel_per_vcpu_second
        if vcpu_second is not UNSET:
            field_dict["vcpuSecond"] = vcpu_second
        if gib_second is not UNSET:
            field_dict["gibSecond"] = gib_second
        if connector_gib is not UNSET:
            field_dict["connectorGib"] = connector_gib
        if storage_gib_month is not UNSET:
            field_dict["storageGibMonth"] = storage_gib_month
        if egress_gib is not UNSET:
            field_dict["egressGib"] = egress_gib

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        version = d.pop("version", UNSET)

        fuel_per_vcpu_second = d.pop("fuelPerVcpuSecond", UNSET)

        vcpu_second = d.pop("vcpuSecond", UNSET)

        gib_second = d.pop("gibSecond", UNSET)

        connector_gib = d.pop("connectorGib", UNSET)

        storage_gib_month = d.pop("storageGibMonth", UNSET)

        egress_gib = d.pop("egressGib", UNSET)

        ops_prices_get_response_200 = cls(
            version=version,
            fuel_per_vcpu_second=fuel_per_vcpu_second,
            vcpu_second=vcpu_second,
            gib_second=gib_second,
            connector_gib=connector_gib,
            storage_gib_month=storage_gib_month,
            egress_gib=egress_gib,
        )

        ops_prices_get_response_200.additional_properties = d
        return ops_prices_get_response_200

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
