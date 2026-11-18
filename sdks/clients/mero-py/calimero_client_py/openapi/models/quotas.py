from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="Quotas")


@_attrs_define
class Quotas:
    """The per-account limits, and the compute and file limits of each organization (or personal account) that has no
    override. `maxFuelPerDeveloperPerDay` (0: no limit) is a decimal string; the others are JSON numbers.

        Attributes:
            max_apps_per_account (int):
            max_bundle_bytes_per_account (int):
            max_api_keys_per_account (int):
            max_orgs_per_account (int):
            max_accounts (int):
            max_file_bytes_per_org (int):
            max_concurrent_runs_per_org (int):
            compute_fuel_per_org (int):
            max_fuel_per_developer_per_day (str): an unsigned 64-bit integer as a decimal string (D20)
    """

    max_apps_per_account: int
    max_bundle_bytes_per_account: int
    max_api_keys_per_account: int
    max_orgs_per_account: int
    max_accounts: int
    max_file_bytes_per_org: int
    max_concurrent_runs_per_org: int
    compute_fuel_per_org: int
    max_fuel_per_developer_per_day: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        max_apps_per_account = self.max_apps_per_account

        max_bundle_bytes_per_account = self.max_bundle_bytes_per_account

        max_api_keys_per_account = self.max_api_keys_per_account

        max_orgs_per_account = self.max_orgs_per_account

        max_accounts = self.max_accounts

        max_file_bytes_per_org = self.max_file_bytes_per_org

        max_concurrent_runs_per_org = self.max_concurrent_runs_per_org

        compute_fuel_per_org = self.compute_fuel_per_org

        max_fuel_per_developer_per_day = self.max_fuel_per_developer_per_day

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "maxAppsPerAccount": max_apps_per_account,
                "maxBundleBytesPerAccount": max_bundle_bytes_per_account,
                "maxApiKeysPerAccount": max_api_keys_per_account,
                "maxOrgsPerAccount": max_orgs_per_account,
                "maxAccounts": max_accounts,
                "maxFileBytesPerOrg": max_file_bytes_per_org,
                "maxConcurrentRunsPerOrg": max_concurrent_runs_per_org,
                "computeFuelPerOrg": compute_fuel_per_org,
                "maxFuelPerDeveloperPerDay": max_fuel_per_developer_per_day,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        max_apps_per_account = d.pop("maxAppsPerAccount")

        max_bundle_bytes_per_account = d.pop("maxBundleBytesPerAccount")

        max_api_keys_per_account = d.pop("maxApiKeysPerAccount")

        max_orgs_per_account = d.pop("maxOrgsPerAccount")

        max_accounts = d.pop("maxAccounts")

        max_file_bytes_per_org = d.pop("maxFileBytesPerOrg")

        max_concurrent_runs_per_org = d.pop("maxConcurrentRunsPerOrg")

        compute_fuel_per_org = d.pop("computeFuelPerOrg")

        max_fuel_per_developer_per_day = d.pop("maxFuelPerDeveloperPerDay")

        quotas = cls(
            max_apps_per_account=max_apps_per_account,
            max_bundle_bytes_per_account=max_bundle_bytes_per_account,
            max_api_keys_per_account=max_api_keys_per_account,
            max_orgs_per_account=max_orgs_per_account,
            max_accounts=max_accounts,
            max_file_bytes_per_org=max_file_bytes_per_org,
            max_concurrent_runs_per_org=max_concurrent_runs_per_org,
            compute_fuel_per_org=compute_fuel_per_org,
            max_fuel_per_developer_per_day=max_fuel_per_developer_per_day,
        )

        quotas.additional_properties = d
        return quotas

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
