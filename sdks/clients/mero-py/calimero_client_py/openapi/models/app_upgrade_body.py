from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.confirm import Confirm


T = TypeVar("T", bound="AppUpgradeBody")


@_attrs_define
class AppUpgradeBody:
    """
    Attributes:
        bundle (Union[Unset, str]):
        package (Union[Unset, str]):
        version (Union[Unset, str]):
        service (Union[Unset, str]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
        migrate (Union[Unset, str]):
    """

    bundle: Union[Unset, str] = UNSET
    package: Union[Unset, str] = UNSET
    version: Union[Unset, str] = UNSET
    service: Union[Unset, str] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET
    migrate: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.confirm import Confirm

        bundle = self.bundle

        package = self.package

        version = self.version

        service = self.service

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        migrate = self.migrate

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if bundle is not UNSET:
            field_dict["bundle"] = bundle
        if package is not UNSET:
            field_dict["package"] = package
        if version is not UNSET:
            field_dict["version"] = version
        if service is not UNSET:
            field_dict["service"] = service
        if confirm is not UNSET:
            field_dict["confirm"] = confirm
        if migrate is not UNSET:
            field_dict["migrate"] = migrate

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.confirm import Confirm

        d = dict(src_dict)
        bundle = d.pop("bundle", UNSET)

        package = d.pop("package", UNSET)

        version = d.pop("version", UNSET)

        service = d.pop("service", UNSET)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        migrate = d.pop("migrate", UNSET)

        app_upgrade_body = cls(
            bundle=bundle,
            package=package,
            version=version,
            service=service,
            confirm=confirm,
            migrate=migrate,
        )

        app_upgrade_body.additional_properties = d
        return app_upgrade_body

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
