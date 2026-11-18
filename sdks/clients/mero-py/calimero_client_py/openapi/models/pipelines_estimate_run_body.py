from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.pipelines_estimate_run_body_counts import (
        PipelinesEstimateRunBodyCounts,
    )
    from ..models.pipelines_estimate_run_body_params import (
        PipelinesEstimateRunBodyParams,
    )
    from ..models.pipelines_estimate_run_body_inputs import (
        PipelinesEstimateRunBodyInputs,
    )


T = TypeVar("T", bound="PipelinesEstimateRunBody")


@_attrs_define
class PipelinesEstimateRunBody:
    """
    Attributes:
        inputs (Union[Unset, PipelinesEstimateRunBodyInputs]):
        counts (Union[Unset, PipelinesEstimateRunBodyCounts]):
        params (Union[Unset, PipelinesEstimateRunBodyParams]):
    """

    inputs: Union[Unset, "PipelinesEstimateRunBodyInputs"] = UNSET
    counts: Union[Unset, "PipelinesEstimateRunBodyCounts"] = UNSET
    params: Union[Unset, "PipelinesEstimateRunBodyParams"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.pipelines_estimate_run_body_counts import (
            PipelinesEstimateRunBodyCounts,
        )
        from ..models.pipelines_estimate_run_body_params import (
            PipelinesEstimateRunBodyParams,
        )
        from ..models.pipelines_estimate_run_body_inputs import (
            PipelinesEstimateRunBodyInputs,
        )

        inputs: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.inputs, Unset):
            inputs = self.inputs.to_dict()

        counts: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.counts, Unset):
            counts = self.counts.to_dict()

        params: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.params, Unset):
            params = self.params.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if inputs is not UNSET:
            field_dict["inputs"] = inputs
        if counts is not UNSET:
            field_dict["counts"] = counts
        if params is not UNSET:
            field_dict["params"] = params

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pipelines_estimate_run_body_counts import (
            PipelinesEstimateRunBodyCounts,
        )
        from ..models.pipelines_estimate_run_body_params import (
            PipelinesEstimateRunBodyParams,
        )
        from ..models.pipelines_estimate_run_body_inputs import (
            PipelinesEstimateRunBodyInputs,
        )

        d = dict(src_dict)
        _inputs = d.pop("inputs", UNSET)
        inputs: Union[Unset, PipelinesEstimateRunBodyInputs]
        if isinstance(_inputs, Unset):
            inputs = UNSET
        else:
            inputs = PipelinesEstimateRunBodyInputs.from_dict(_inputs)

        _counts = d.pop("counts", UNSET)
        counts: Union[Unset, PipelinesEstimateRunBodyCounts]
        if isinstance(_counts, Unset):
            counts = UNSET
        else:
            counts = PipelinesEstimateRunBodyCounts.from_dict(_counts)

        _params = d.pop("params", UNSET)
        params: Union[Unset, PipelinesEstimateRunBodyParams]
        if isinstance(_params, Unset):
            params = UNSET
        else:
            params = PipelinesEstimateRunBodyParams.from_dict(_params)

        pipelines_estimate_run_body = cls(
            inputs=inputs,
            counts=counts,
            params=params,
        )

        return pipelines_estimate_run_body
