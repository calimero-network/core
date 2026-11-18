from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PackagePipelinesResponse200AdditionalProperty")


@_attrs_define
class PackagePipelinesResponse200AdditionalProperty:
    """
    Attributes:
        hash_ (str):
        worst_case_fuel (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it
            is).
        modules (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
        pipeline (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
    """

    hash_: str
    worst_case_fuel: Any
    modules: Any
    pipeline: Any
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        hash_ = self.hash_

        worst_case_fuel = self.worst_case_fuel

        modules = self.modules

        pipeline = self.pipeline

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "hash": hash_,
                "worstCaseFuel": worst_case_fuel,
                "modules": modules,
                "pipeline": pipeline,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        hash_ = d.pop("hash")

        worst_case_fuel = d.pop("worstCaseFuel")

        modules = d.pop("modules")

        pipeline = d.pop("pipeline")

        package_pipelines_response_200_additional_property = cls(
            hash_=hash_,
            worst_case_fuel=worst_case_fuel,
            modules=modules,
            pipeline=pipeline,
        )

        package_pipelines_response_200_additional_property.additional_properties = d
        return package_pipelines_response_200_additional_property

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
