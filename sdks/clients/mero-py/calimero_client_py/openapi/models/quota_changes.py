from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.operator_proof import OperatorProof


T = TypeVar("T", bound="QuotaChanges")


@_attrs_define
class QuotaChanges:
    """The limits `POST /ops/quotas` changes, each optional: without `org`, the cluster's (those `GET /ops/quotas` answers,
    `maxFuelPerDeveloperPerDay` a decimal string); with `org`, one payer's (`maxConcurrentRuns`, `computeFuel` a decimal
    string, `maxFileBytes`; `null` clears one). `orgs` is ignored. Any other property is refused `400 UNKNOWN_QUOTA`.

        Attributes:
            org (Union[Unset, str]):
            operator_proof (Union[Unset, OperatorProof]): The operator key's Ed25519 signature (hex) over the statement a
                `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.
            orgs (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for whose
                it is).
            max_apps_per_account (Union[Unset, int]):
            max_bundle_bytes_per_account (Union[Unset, int]):
            max_api_keys_per_account (Union[Unset, int]):
            max_orgs_per_account (Union[Unset, int]):
            max_accounts (Union[Unset, int]):
            max_file_bytes_per_org (Union[Unset, int]):
            max_concurrent_runs_per_org (Union[Unset, int]):
            compute_fuel_per_org (Union[Unset, int]):
            max_fuel_per_developer_per_day (Union[Unset, str]): an unsigned 64-bit integer as a decimal string (D20)
            max_concurrent_runs (Union[None, Unset, int]):
            compute_fuel (Union[None, Unset, str]): an unsigned 64-bit integer as a decimal string (D20)
            max_file_bytes (Union[None, Unset, int]):
    """

    org: Union[Unset, str] = UNSET
    operator_proof: Union[Unset, "OperatorProof"] = UNSET
    orgs: Union[Unset, Any] = UNSET
    max_apps_per_account: Union[Unset, int] = UNSET
    max_bundle_bytes_per_account: Union[Unset, int] = UNSET
    max_api_keys_per_account: Union[Unset, int] = UNSET
    max_orgs_per_account: Union[Unset, int] = UNSET
    max_accounts: Union[Unset, int] = UNSET
    max_file_bytes_per_org: Union[Unset, int] = UNSET
    max_concurrent_runs_per_org: Union[Unset, int] = UNSET
    compute_fuel_per_org: Union[Unset, int] = UNSET
    max_fuel_per_developer_per_day: Union[Unset, str] = UNSET
    max_concurrent_runs: Union[None, Unset, int] = UNSET
    compute_fuel: Union[None, Unset, str] = UNSET
    max_file_bytes: Union[None, Unset, int] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.operator_proof import OperatorProof

        org = self.org

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        orgs = self.orgs

        max_apps_per_account = self.max_apps_per_account

        max_bundle_bytes_per_account = self.max_bundle_bytes_per_account

        max_api_keys_per_account = self.max_api_keys_per_account

        max_orgs_per_account = self.max_orgs_per_account

        max_accounts = self.max_accounts

        max_file_bytes_per_org = self.max_file_bytes_per_org

        max_concurrent_runs_per_org = self.max_concurrent_runs_per_org

        compute_fuel_per_org = self.compute_fuel_per_org

        max_fuel_per_developer_per_day = self.max_fuel_per_developer_per_day

        max_concurrent_runs: Union[None, Unset, int]
        if isinstance(self.max_concurrent_runs, Unset):
            max_concurrent_runs = UNSET
        else:
            max_concurrent_runs = self.max_concurrent_runs

        compute_fuel: Union[None, Unset, str]
        if isinstance(self.compute_fuel, Unset):
            compute_fuel = UNSET
        else:
            compute_fuel = self.compute_fuel

        max_file_bytes: Union[None, Unset, int]
        if isinstance(self.max_file_bytes, Unset):
            max_file_bytes = UNSET
        else:
            max_file_bytes = self.max_file_bytes

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if org is not UNSET:
            field_dict["org"] = org
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof
        if orgs is not UNSET:
            field_dict["orgs"] = orgs
        if max_apps_per_account is not UNSET:
            field_dict["maxAppsPerAccount"] = max_apps_per_account
        if max_bundle_bytes_per_account is not UNSET:
            field_dict["maxBundleBytesPerAccount"] = max_bundle_bytes_per_account
        if max_api_keys_per_account is not UNSET:
            field_dict["maxApiKeysPerAccount"] = max_api_keys_per_account
        if max_orgs_per_account is not UNSET:
            field_dict["maxOrgsPerAccount"] = max_orgs_per_account
        if max_accounts is not UNSET:
            field_dict["maxAccounts"] = max_accounts
        if max_file_bytes_per_org is not UNSET:
            field_dict["maxFileBytesPerOrg"] = max_file_bytes_per_org
        if max_concurrent_runs_per_org is not UNSET:
            field_dict["maxConcurrentRunsPerOrg"] = max_concurrent_runs_per_org
        if compute_fuel_per_org is not UNSET:
            field_dict["computeFuelPerOrg"] = compute_fuel_per_org
        if max_fuel_per_developer_per_day is not UNSET:
            field_dict["maxFuelPerDeveloperPerDay"] = max_fuel_per_developer_per_day
        if max_concurrent_runs is not UNSET:
            field_dict["maxConcurrentRuns"] = max_concurrent_runs
        if compute_fuel is not UNSET:
            field_dict["computeFuel"] = compute_fuel
        if max_file_bytes is not UNSET:
            field_dict["maxFileBytes"] = max_file_bytes

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.operator_proof import OperatorProof

        d = dict(src_dict)
        org = d.pop("org", UNSET)

        _operator_proof = d.pop("operatorProof", UNSET)
        operator_proof: Union[Unset, OperatorProof]
        if isinstance(_operator_proof, Unset):
            operator_proof = UNSET
        else:
            operator_proof = OperatorProof.from_dict(_operator_proof)

        orgs = d.pop("orgs", UNSET)

        max_apps_per_account = d.pop("maxAppsPerAccount", UNSET)

        max_bundle_bytes_per_account = d.pop("maxBundleBytesPerAccount", UNSET)

        max_api_keys_per_account = d.pop("maxApiKeysPerAccount", UNSET)

        max_orgs_per_account = d.pop("maxOrgsPerAccount", UNSET)

        max_accounts = d.pop("maxAccounts", UNSET)

        max_file_bytes_per_org = d.pop("maxFileBytesPerOrg", UNSET)

        max_concurrent_runs_per_org = d.pop("maxConcurrentRunsPerOrg", UNSET)

        compute_fuel_per_org = d.pop("computeFuelPerOrg", UNSET)

        max_fuel_per_developer_per_day = d.pop("maxFuelPerDeveloperPerDay", UNSET)

        def _parse_max_concurrent_runs(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_concurrent_runs = _parse_max_concurrent_runs(
            d.pop("maxConcurrentRuns", UNSET)
        )

        def _parse_compute_fuel(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        compute_fuel = _parse_compute_fuel(d.pop("computeFuel", UNSET))

        def _parse_max_file_bytes(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_file_bytes = _parse_max_file_bytes(d.pop("maxFileBytes", UNSET))

        quota_changes = cls(
            org=org,
            operator_proof=operator_proof,
            orgs=orgs,
            max_apps_per_account=max_apps_per_account,
            max_bundle_bytes_per_account=max_bundle_bytes_per_account,
            max_api_keys_per_account=max_api_keys_per_account,
            max_orgs_per_account=max_orgs_per_account,
            max_accounts=max_accounts,
            max_file_bytes_per_org=max_file_bytes_per_org,
            max_concurrent_runs_per_org=max_concurrent_runs_per_org,
            compute_fuel_per_org=compute_fuel_per_org,
            max_fuel_per_developer_per_day=max_fuel_per_developer_per_day,
            max_concurrent_runs=max_concurrent_runs,
            compute_fuel=compute_fuel,
            max_file_bytes=max_file_bytes,
        )

        return quota_changes
