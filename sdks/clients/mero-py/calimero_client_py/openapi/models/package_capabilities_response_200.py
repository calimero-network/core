from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

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
        abi (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
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
    abi: Any
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
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

        abi = self.abi

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
                "abi": abi,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
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

        abi = d.pop("abi")

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
            abi=abi,
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
