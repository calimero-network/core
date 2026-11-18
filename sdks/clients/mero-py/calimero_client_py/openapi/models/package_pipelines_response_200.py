from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.package_pipelines_response_200_additional_property import (
        PackagePipelinesResponse200AdditionalProperty,
    )


T = TypeVar("T", bound="PackagePipelinesResponse200")


@_attrs_define
class PackagePipelinesResponse200:
    """ """

    additional_properties: dict[
        str, "PackagePipelinesResponse200AdditionalProperty"
    ] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.package_pipelines_response_200_additional_property import (
            PackagePipelinesResponse200AdditionalProperty,
        )

        field_dict: dict[str, Any] = {}
        for prop_name, prop in self.additional_properties.items():
            field_dict[prop_name] = prop.to_dict()

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.package_pipelines_response_200_additional_property import (
            PackagePipelinesResponse200AdditionalProperty,
        )

        d = dict(src_dict)
        package_pipelines_response_200 = cls()

        additional_properties = {}
        for prop_name, prop_dict in d.items():
            additional_property = (
                PackagePipelinesResponse200AdditionalProperty.from_dict(prop_dict)
            )

            additional_properties[prop_name] = additional_property

        package_pipelines_response_200.additional_properties = additional_properties
        return package_pipelines_response_200

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> "PackagePipelinesResponse200AdditionalProperty":
        return self.additional_properties[key]

    def __setitem__(
        self, key: str, value: "PackagePipelinesResponse200AdditionalProperty"
    ) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
