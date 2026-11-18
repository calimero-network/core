from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.quotas import Quotas
    from ..models.usage_response_200_file_bytes import UsageResponse200FileBytes


T = TypeVar("T", bound="UsageResponse200")


@_attrs_define
class UsageResponse200:
    """
    Attributes:
        apps (int):
        bundle_bytes (str): an unsigned 64-bit integer as a decimal string (D20)
        api_keys (int):
        orgs_owned (int):
        file_bytes (UsageResponse200FileBytes):
        quotas (Quotas): The per-account limits, and the compute and file limits of each organization (or personal
            account) that has no override. `maxFuelPerDeveloperPerDay` (0: no limit) is a decimal string; the others are
            JSON numbers.
        developer_fuel_today (str): an unsigned 64-bit integer as a decimal string (D20)
        developer_writes_today (str): an unsigned 64-bit integer as a decimal string (D20)
    """

    apps: int
    bundle_bytes: str
    api_keys: int
    orgs_owned: int
    file_bytes: "UsageResponse200FileBytes"
    quotas: "Quotas"
    developer_fuel_today: str
    developer_writes_today: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.quotas import Quotas
        from ..models.usage_response_200_file_bytes import UsageResponse200FileBytes

        apps = self.apps

        bundle_bytes = self.bundle_bytes

        api_keys = self.api_keys

        orgs_owned = self.orgs_owned

        file_bytes = self.file_bytes.to_dict()

        quotas = self.quotas.to_dict()

        developer_fuel_today = self.developer_fuel_today

        developer_writes_today = self.developer_writes_today

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "apps": apps,
                "bundleBytes": bundle_bytes,
                "apiKeys": api_keys,
                "orgsOwned": orgs_owned,
                "fileBytes": file_bytes,
                "quotas": quotas,
                "developerFuelToday": developer_fuel_today,
                "developerWritesToday": developer_writes_today,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.quotas import Quotas
        from ..models.usage_response_200_file_bytes import UsageResponse200FileBytes

        d = dict(src_dict)
        apps = d.pop("apps")

        bundle_bytes = d.pop("bundleBytes")

        api_keys = d.pop("apiKeys")

        orgs_owned = d.pop("orgsOwned")

        file_bytes = UsageResponse200FileBytes.from_dict(d.pop("fileBytes"))

        quotas = Quotas.from_dict(d.pop("quotas"))

        developer_fuel_today = d.pop("developerFuelToday")

        developer_writes_today = d.pop("developerWritesToday")

        usage_response_200 = cls(
            apps=apps,
            bundle_bytes=bundle_bytes,
            api_keys=api_keys,
            orgs_owned=orgs_owned,
            file_bytes=file_bytes,
            quotas=quotas,
            developer_fuel_today=developer_fuel_today,
            developer_writes_today=developer_writes_today,
        )

        usage_response_200.additional_properties = d
        return usage_response_200

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
