Ada Example, Lin Example

## Abstract

We study reliable queues that preserve work when a consumer fails. The design uses explicit acknowledgements and bounded retries.

## Introduction

A queue is useful only when its delivery contract is clear. This paper compares retry policies and explains how operators can inspect pending work.

## Results

The measured system keeps ordering within each partition and recovers after a worker restart.
$R=1-p$
