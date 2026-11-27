from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.release_pipeline_modules import ReleasePipelineModules


T = TypeVar("T", bound="ReleasePipeline")


@_attrs_define
class ReleasePipeline:
    """A registered pipeline: its hash, worst-case fuel (a decimal string), the module releases it pins, and its canonical
    JSON.

        Attributes:
            hash_ (str):
            worst_case_fuel (str):
            modules (ReleasePipelineModules):
            pipeline (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
    """

    hash_: str
    worst_case_fuel: str
    modules: "ReleasePipelineModules"
    pipeline: Any
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.release_pipeline_modules import ReleasePipelineModules

        hash_ = self.hash_

        worst_case_fuel = self.worst_case_fuel

        modules = self.modules.to_dict()

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
        from ..models.release_pipeline_modules import ReleasePipelineModules

        d = dict(src_dict)
        hash_ = d.pop("hash")

        worst_case_fuel = d.pop("worstCaseFuel")

        modules = ReleasePipelineModules.from_dict(d.pop("modules"))

        pipeline = d.pop("pipeline")

        release_pipeline = cls(
            hash_=hash_,
            worst_case_fuel=worst_case_fuel,
            modules=modules,
            pipeline=pipeline,
        )

        release_pipeline.additional_properties = d
        return release_pipeline

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
