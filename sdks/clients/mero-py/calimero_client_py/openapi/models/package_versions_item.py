from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.package_versions_item_source import PackageVersionsItemSource
from ..models.package_versions_item_visibility import PackageVersionsItemVisibility
from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.package_versions_item_orgs_item import PackageVersionsItemOrgsItem
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
        visibility (Union[Unset, PackageVersionsItemVisibility]):
        orgs (Union[Unset, list['PackageVersionsItemOrgsItem']]):
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
    visibility: Union[Unset, PackageVersionsItemVisibility] = UNSET
    orgs: Union[Unset, list["PackageVersionsItemOrgsItem"]] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.package_versions_item_orgs_item import PackageVersionsItemOrgsItem
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

        visibility: Union[Unset, str] = UNSET
        if not isinstance(self.visibility, Unset):
            visibility = self.visibility.value

        orgs: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.orgs, Unset):
            orgs = []
            for orgs_item_data in self.orgs:
                orgs_item = orgs_item_data.to_dict()
                orgs.append(orgs_item)

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
        if visibility is not UNSET:
            field_dict["visibility"] = visibility
        if orgs is not UNSET:
            field_dict["orgs"] = orgs

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.package_versions_item_orgs_item import PackageVersionsItemOrgsItem
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

        _visibility = d.pop("visibility", UNSET)
        visibility: Union[Unset, PackageVersionsItemVisibility]
        if isinstance(_visibility, Unset):
            visibility = UNSET
        else:
            visibility = PackageVersionsItemVisibility(_visibility)

        orgs = []
        _orgs = d.pop("orgs", UNSET)
        for orgs_item_data in _orgs or []:
            orgs_item = PackageVersionsItemOrgsItem.from_dict(orgs_item_data)

            orgs.append(orgs_item)

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
            visibility=visibility,
            orgs=orgs,
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
