from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

T = TypeVar("T", bound="UnitMember")


@_attrs_define
class UnitMember:
    """An app of a unit, by its path from the instance: its app id (absent on a release's card, where no app exists), the
    release it runs or would run, that release's signer, and the destinations it alone may send data to.

        Attributes:
            package (str):
            version (str):
            signer (str):
            destinations (list[str]):
            app (Union[Unset, str]):
    """

    package: str
    version: str
    signer: str
    destinations: list[str]
    app: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        package = self.package

        version = self.version

        signer = self.signer

        destinations = self.destinations

        app = self.app

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "version": version,
                "signer": signer,
                "destinations": destinations,
            }
        )
        if app is not UNSET:
            field_dict["app"] = app

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        package = d.pop("package")

        version = d.pop("version")

        signer = d.pop("signer")

        destinations = cast(list[str], d.pop("destinations"))

        app = d.pop("app", UNSET)

        unit_member = cls(
            package=package,
            version=version,
            signer=signer,
            destinations=destinations,
            app=app,
        )

        unit_member.additional_properties = d
        return unit_member

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
