from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.consent_page_create_body_type_18_subject_new_org import (
        ConsentPageCreateBodyType18SubjectNewOrg,
    )


T = TypeVar("T", bound="ConsentPageCreateBodyType18Subject")


@_attrs_define
class ConsentPageCreateBodyType18Subject:
    """
    Attributes:
        package (str):
        name (str):
        version (Union[Unset, str]):
        deployment (Union[Unset, str]):
        org (Union[Unset, str]):
        new_org (Union[Unset, ConsentPageCreateBodyType18SubjectNewOrg]):
        init_args (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for
            whose it is).
    """

    package: str
    name: str
    version: Union[Unset, str] = UNSET
    deployment: Union[Unset, str] = UNSET
    org: Union[Unset, str] = UNSET
    new_org: Union[Unset, "ConsentPageCreateBodyType18SubjectNewOrg"] = UNSET
    init_args: Union[Unset, Any] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.consent_page_create_body_type_18_subject_new_org import (
            ConsentPageCreateBodyType18SubjectNewOrg,
        )

        package = self.package

        name = self.name

        version = self.version

        deployment = self.deployment

        org = self.org

        new_org: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.new_org, Unset):
            new_org = self.new_org.to_dict()

        init_args = self.init_args

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "name": name,
            }
        )
        if version is not UNSET:
            field_dict["version"] = version
        if deployment is not UNSET:
            field_dict["deployment"] = deployment
        if org is not UNSET:
            field_dict["org"] = org
        if new_org is not UNSET:
            field_dict["newOrg"] = new_org
        if init_args is not UNSET:
            field_dict["initArgs"] = init_args

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.consent_page_create_body_type_18_subject_new_org import (
            ConsentPageCreateBodyType18SubjectNewOrg,
        )

        d = dict(src_dict)
        package = d.pop("package")

        name = d.pop("name")

        version = d.pop("version", UNSET)

        deployment = d.pop("deployment", UNSET)

        org = d.pop("org", UNSET)

        _new_org = d.pop("newOrg", UNSET)
        new_org: Union[Unset, ConsentPageCreateBodyType18SubjectNewOrg]
        if isinstance(_new_org, Unset):
            new_org = UNSET
        else:
            new_org = ConsentPageCreateBodyType18SubjectNewOrg.from_dict(_new_org)

        init_args = d.pop("initArgs", UNSET)

        consent_page_create_body_type_18_subject = cls(
            package=package,
            name=name,
            version=version,
            deployment=deployment,
            org=org,
            new_org=new_org,
            init_args=init_args,
        )

        consent_page_create_body_type_18_subject.additional_properties = d
        return consent_page_create_body_type_18_subject

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
