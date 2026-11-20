from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.package_capabilities_response_200_members_type_0 import (
        PackageCapabilitiesResponse200MembersType0,
    )


T = TypeVar("T", bound="PackageCapabilitiesResponse200")


@_attrs_define
class PackageCapabilitiesResponse200:
    """
    Attributes:
        package (str):
        version (str):
        service (Union[None, str]):
        bundle (str):
        mpk (str):
        manifest_hash (str):
        signer (str):
        name (Union[None, str]):
        author (Union[None, str]):
        outbound (list[str]):
        secrets (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
        uses (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
        abi (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
        destinations (Union[None, list[str]]):
        members (Union['PackageCapabilitiesResponse200MembersType0', None]):
    """

    package: str
    version: str
    service: Union[None, str]
    bundle: str
    mpk: str
    manifest_hash: str
    signer: str
    name: Union[None, str]
    author: Union[None, str]
    outbound: list[str]
    secrets: Any
    uses: Any
    abi: Any
    destinations: Union[None, list[str]]
    members: Union["PackageCapabilitiesResponse200MembersType0", None]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.package_capabilities_response_200_members_type_0 import (
            PackageCapabilitiesResponse200MembersType0,
        )

        package = self.package

        version = self.version

        service: Union[None, str]
        service = self.service

        bundle = self.bundle

        mpk = self.mpk

        manifest_hash = self.manifest_hash

        signer = self.signer

        name: Union[None, str]
        name = self.name

        author: Union[None, str]
        author = self.author

        outbound = self.outbound

        secrets = self.secrets

        uses = self.uses

        abi = self.abi

        destinations: Union[None, list[str]]
        if isinstance(self.destinations, list):
            destinations = self.destinations

        else:
            destinations = self.destinations

        members: Union[None, dict[str, Any]]
        if isinstance(self.members, PackageCapabilitiesResponse200MembersType0):
            members = self.members.to_dict()
        else:
            members = self.members

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "version": version,
                "service": service,
                "bundle": bundle,
                "mpk": mpk,
                "manifestHash": manifest_hash,
                "signer": signer,
                "name": name,
                "author": author,
                "outbound": outbound,
                "secrets": secrets,
                "uses": uses,
                "abi": abi,
                "destinations": destinations,
                "members": members,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.package_capabilities_response_200_members_type_0 import (
            PackageCapabilitiesResponse200MembersType0,
        )

        d = dict(src_dict)
        package = d.pop("package")

        version = d.pop("version")

        def _parse_service(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        service = _parse_service(d.pop("service"))

        bundle = d.pop("bundle")

        mpk = d.pop("mpk")

        manifest_hash = d.pop("manifestHash")

        signer = d.pop("signer")

        def _parse_name(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        name = _parse_name(d.pop("name"))

        def _parse_author(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        author = _parse_author(d.pop("author"))

        outbound = cast(list[str], d.pop("outbound"))

        secrets = d.pop("secrets")

        uses = d.pop("uses")

        abi = d.pop("abi")

        def _parse_destinations(data: object) -> Union[None, list[str]]:
            if data is None:
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                destinations_type_0 = cast(list[str], data)

                return destinations_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, list[str]], data)

        destinations = _parse_destinations(d.pop("destinations"))

        def _parse_members(
            data: object,
        ) -> Union["PackageCapabilitiesResponse200MembersType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                members_type_0 = PackageCapabilitiesResponse200MembersType0.from_dict(
                    data
                )

                return members_type_0
            except:  # noqa: E722
                pass
            return cast(Union["PackageCapabilitiesResponse200MembersType0", None], data)

        members = _parse_members(d.pop("members"))

        package_capabilities_response_200 = cls(
            package=package,
            version=version,
            service=service,
            bundle=bundle,
            mpk=mpk,
            manifest_hash=manifest_hash,
            signer=signer,
            name=name,
            author=author,
            outbound=outbound,
            secrets=secrets,
            uses=uses,
            abi=abi,
            destinations=destinations,
            members=members,
        )

        package_capabilities_response_200.additional_properties = d
        return package_capabilities_response_200

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
