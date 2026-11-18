from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.quotas import Quotas


T = TypeVar("T", bound="OrgUsageResponse200")


@_attrs_define
class OrgUsageResponse200:
    """
    Attributes:
        apps (int):
        developer_fuel_today (str): an unsigned 64-bit integer as a decimal string (D20)
        developer_writes_today (str): an unsigned 64-bit integer as a decimal string (D20)
        quotas (Quotas): The per-account limits, and the compute and file limits of each organization (or personal
            account) that has no override. `maxFuelPerDeveloperPerDay` (0: no limit) is a decimal string; the others are
            JSON numbers.
    """

    apps: int
    developer_fuel_today: str
    developer_writes_today: str
    quotas: "Quotas"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.quotas import Quotas

        apps = self.apps

        developer_fuel_today = self.developer_fuel_today

        developer_writes_today = self.developer_writes_today

        quotas = self.quotas.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "apps": apps,
                "developerFuelToday": developer_fuel_today,
                "developerWritesToday": developer_writes_today,
                "quotas": quotas,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.quotas import Quotas

        d = dict(src_dict)
        apps = d.pop("apps")

        developer_fuel_today = d.pop("developerFuelToday")

        developer_writes_today = d.pop("developerWritesToday")

        quotas = Quotas.from_dict(d.pop("quotas"))

        org_usage_response_200 = cls(
            apps=apps,
            developer_fuel_today=developer_fuel_today,
            developer_writes_today=developer_writes_today,
            quotas=quotas,
        )

        org_usage_response_200.additional_properties = d
        return org_usage_response_200

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
