from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.package_versions_item_source import PackageVersionsItemSource
from typing import cast

if TYPE_CHECKING:
    from ..models.package_versions_item_services import PackageVersionsItemServices


T = TypeVar("T", bound="PackageVersionsItem")


@_attrs_define
class PackageVersionsItem:
    """
    Attributes:
        version (str):
        mpk (str):
        size (int):
        source (PackageVersionsItemSource):
        services (PackageVersionsItemServices):
        metadata (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
        outbound (list[str]):
        installed_by (str):
        installed_at (int):
    """

    version: str
    mpk: str
    size: int
    source: PackageVersionsItemSource
    services: "PackageVersionsItemServices"
    metadata: Any
    outbound: list[str]
    installed_by: str
    installed_at: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.package_versions_item_services import PackageVersionsItemServices

        version = self.version

        mpk = self.mpk

        size = self.size

        source = self.source.value

        services = self.services.to_dict()

        metadata = self.metadata

        outbound = self.outbound

        installed_by = self.installed_by

        installed_at = self.installed_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "version": version,
                "mpk": mpk,
                "size": size,
                "source": source,
                "services": services,
                "metadata": metadata,
                "outbound": outbound,
                "installedBy": installed_by,
                "installedAt": installed_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.package_versions_item_services import PackageVersionsItemServices

        d = dict(src_dict)
        version = d.pop("version")

        mpk = d.pop("mpk")

        size = d.pop("size")

        source = PackageVersionsItemSource(d.pop("source"))

        services = PackageVersionsItemServices.from_dict(d.pop("services"))

        metadata = d.pop("metadata")

        outbound = cast(list[str], d.pop("outbound"))

        installed_by = d.pop("installedBy")

        installed_at = d.pop("installedAt")

        package_versions_item = cls(
            version=version,
            mpk=mpk,
            size=size,
            source=source,
            services=services,
            metadata=metadata,
            outbound=outbound,
            installed_by=installed_by,
            installed_at=installed_at,
        )

        package_versions_item.additional_properties = d
        return package_versions_item

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
